#!/bin/sh
# Install porter: the three daemons, their systemd user units and D-Bus activation files, the
# helper that shows drives in ~/Cloud with its socket, the provider files and the caller table.
#
#   scripts/install.sh [--prefix DIR] [--destdir DIR] [--sysconfdir DIR] [--bin-dir DIR] [--no-build]
#
# --prefix      where the software goes (default /usr; the environment variable PREFIX also sets it)
# --destdir     a staging root put in front of every path, for packagers and for trying the install
#               (default none; the environment variable DESTDIR also sets it)
# --sysconfdir  where the caller table goes (default /etc)
# --bin-dir     install the already built daemons found in DIR instead of building them
# --no-build    install from target/release (or $CARGO_TARGET_DIR/release) without building first
#               (building needs git and cargo and is refused as root: build as your user, then
#               install with --no-build as root)
#
# What goes where (DESTDIR, when given, is in front of all of it):
#   PREFIX/libexec/quire/{accountd,syncd,inferd}             the daemons
#   PREFIX/libexec/quire/porter-mountd                       the helper syncd asks to show a drive in
#                                                            ~/Cloud (runs fusermount3, which syncd's
#                                                            sandbox refuses)
#   PREFIX/lib/systemd/user/porter-mountd.{service,socket}   its user units; the socket is enabled for
#                                                            every user by the link
#                                                            sockets.target.wants/porter-mountd.socket
#   PREFIX/lib/systemd/user/{accountd,syncd,inferd}.service  the user units
#   PREFIX/lib/systemd/user/{accountd,syncd,inferd}.service.d/sandbox.conf
#                                                            the part of each unit's sandbox that needs a
#                                                            mount namespace; left out (with one line saying
#                                                            so) when a probe shows this system refuses a user
#                                                            service one (Ubuntu 24.04's AppArmor restriction
#                                                            on user namespaces); always installed with a destdir
#   PREFIX/share/dbus-1/services/org.quire.*.service         D-Bus activation
#   PREFIX/share/porter/providers/*.toml                     the provider files
#   PREFIX/share/porter/clients.toml                         only when dist/clients.toml exists
#   PREFIX/share/porter/inferd-tailnet.conf                  the tailnet drop-in, as data: the Settings switch
#                                                            installs it for the person; never installed live
#   PREFIX/share/quire/settings/inferd.settings.toml         the AI settings page schema
#   PREFIX/share/doc/porter/examples/                        inferd.toml and inferd-cloud.conf: samples
#                                                            for the person to copy; never installed live
#   SYSCONFDIR/porter/{callers,inferd-callers}.toml          the caller tables (kept if the owner changed them)
#
# The script is standalone: it knows nothing of any desktop. Run again, it changes nothing that is
# already as it should be (a file is replaced only when its content differs). It writes nothing under
# a home directory, and only writes under /etc or /usr when run, without a destdir, by someone who
# may. The daemons are built with their default features only.
set -eu

prefix=${PREFIX:-/usr}
destdir=${DESTDIR:-}
sysconfdir=/etc
bin_dir=
build=yes

usage() {
    sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case $1 in
        --prefix) prefix=${2:?--prefix needs a directory}; shift 2 ;;
        --prefix=*) prefix=${1#--prefix=}; shift ;;
        --destdir) destdir=${2:?--destdir needs a directory}; shift 2 ;;
        --destdir=*) destdir=${1#--destdir=}; shift ;;
        --sysconfdir) sysconfdir=${2:?--sysconfdir needs a directory}; shift 2 ;;
        --sysconfdir=*) sysconfdir=${1#--sysconfdir=}; shift ;;
        --bin-dir) bin_dir=${2:?--bin-dir needs a directory}; shift 2 ;;
        --bin-dir=*) bin_dir=${1#--bin-dir=}; shift ;;
        --no-build) build=no; shift ;;
        -h | --help) usage; exit 0 ;;
        *) echo "install.sh: unknown option $1 (try --help)" >&2; exit 2 ;;
    esac
done

# A prefix is absolute and has no trailing slash (the root itself is the empty string).
case $prefix in
    /*) ;;
    *) echo "install.sh: the prefix must be an absolute path, not '$prefix'" >&2; exit 2 ;;
esac
prefix=${prefix%/}
destdir=${destdir%/}

root=$(cd "$(dirname "$0")/.." && pwd)
dist=$root/dist
changed=0

# put MODE SOURCE TARGET [FROM TO]: write SOURCE to DESTDIR/TARGET with mode MODE, replacing the
# text FROM by TO on the way when given. An existing target with the same content is left alone.
put() {
    mode=$1
    source=$2
    target=$destdir$3
    mkdir -p "$(dirname "$target")"
    staged=$target.new.$$
    if [ $# -ge 5 ]; then
        FROM=$4 TO=$5 awk '
            BEGIN { from = ENVIRON["FROM"]; to = ENVIRON["TO"]; n = length(from) }
            {
                out = ""; line = $0
                while ((i = index(line, from)) > 0) {
                    out = out substr(line, 1, i - 1) to
                    line = substr(line, i + n)
                }
                print out line
            }' "$source" >"$staged"
    else
        cp "$source" "$staged"
    fi
    chmod "$mode" "$staged"
    if [ -f "$target" ] && cmp -s "$staged" "$target" && [ "$(mode_of "$target")" = "$(mode_of "$staged")" ]; then
        rm -f "$staged"
    else
        mv -f "$staged" "$target"
        changed=$((changed + 1))
        echo "installed $3"
    fi
}

# link TARGET PATH: DESTDIR/PATH is a symbolic link to TARGET. One already so is left alone.
link() {
    path=$destdir$2
    if [ -L "$path" ] && [ "$(readlink "$path")" = "$1" ]; then
        return
    fi
    mkdir -p "$(dirname "$path")"
    rm -f "$path"
    ln -s "$1" "$path"
    changed=$((changed + 1))
    echo "installed $2"
}

mode_of() {
    stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"
}

# Whether this system lets a user service have its own mount namespace (some refuse it: Ubuntu
# 24.04 restricts unprivileged user namespaces, and such a unit fails with status 226/NAMESPACE).
# With a destdir (packaging) nothing is probed: the packager's target is another computer.
mount_namespace_for_user_units() {
    [ -n "$destdir" ] && return 0
    command -v systemd-run >/dev/null 2>&1 || return 1
    systemd-run --user --quiet --wait -p PrivateTmp=yes -p ProtectSystem=strict true >/dev/null 2>&1
}

# --- The daemons ---------------------------------------------------------------------------------
daemons="accountd syncd inferd"
if [ -n "$bin_dir" ]; then
    built=$bin_dir
else
    # A relative CARGO_TARGET_DIR means the directory the script was started from, which the
    # build below (run from /) would not see; make it absolute once, for the build and the lookup.
    target_dir=${CARGO_TARGET_DIR:-$root/target}
    case $target_dir in
        /*) ;;
        *) target_dir=$(pwd)/$target_dir ;;
    esac
    if [ "$build" = yes ]; then
        # Built by the person, not by root: a build under sudo leaves a root-owned target/ and
        # ~/.cargo behind. Installing what was built is what needs root (system paths).
        if [ "$(id -u)" -eq 0 ]; then
            echo "install.sh: do not build as root; build it as yourself first, then install with --no-build as root" >&2
            exit 1
        fi
        for tool in git cargo; do
            if ! command -v "$tool" >/dev/null 2>&1; then
                echo "install.sh: $tool is needed to build porter; install $tool and run this again" >&2
                exit 1
            fi
        done
        # Linked by this system's own linker. The source tree's .cargo/config.toml links with
        # clang and mold, which is how porter is worked on, not something an install may ask the
        # person to have. Cargo reads that file (and rustup the toolchain file) from the
        # directory it runs in, so the build runs from / with the manifest named, the pinned
        # toolchain passed on, and the one setting an install wants from that file given again.
        toolchain=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$root/rust-toolchain.toml" 2>/dev/null)
        (
            cd / &&
                CARGO_TARGET_DIR=$target_dir &&
                export CARGO_TARGET_DIR &&
                if [ -n "$toolchain" ] && command -v rustup >/dev/null 2>&1; then
                    # rustup 1.28 and later no longer fetches the toolchain RUSTUP_TOOLCHAIN names.
                    rustup run "$toolchain" true >/dev/null 2>&1 ||
                        rustup toolchain install "$toolchain" --profile minimal
                    RUSTUP_TOOLCHAIN=$toolchain
                    export RUSTUP_TOOLCHAIN
                fi &&
                cargo build --release --manifest-path "$root/Cargo.toml" \
                    --config net.git-fetch-with-cli=true -p accountd -p syncd -p inferd -p porter-mountd
        )
    fi
    built=$target_dir/release
fi
for daemon in $daemons; do
    if [ ! -x "$built/$daemon" ]; then
        echo "install.sh: $built/$daemon is not there; build first or point --bin-dir at the daemons" >&2
        exit 1
    fi
done

# The helper that shows drives in ~/Cloud, built beside the daemons. It is not a daemon on the bus
# (its socket starts it), so it is not in the list above. The helper that installs an engine for
# everyone is stoker's (its scripts/install.sh puts it and its polkit action in place).
for helper in porter-mountd; do
    if [ ! -x "$built/$helper" ]; then
        echo "install.sh: $built/$helper is not there; build first or point --bin-dir at it" >&2
        exit 1
    fi
done

# The files in dist/ name /usr/libexec/quire; a different prefix changes that one path.
libexec=$prefix/libexec/quire
for daemon in $daemons; do
    put 755 "$built/$daemon" "$libexec/$daemon"
    put 644 "$dist/$daemon.service" "$prefix/lib/systemd/user/$daemon.service" /usr/libexec/quire "$libexec"
done
if mount_namespace_for_user_units; then
    for daemon in $daemons; do
        put 644 "$dist/$daemon.service.d/sandbox.conf" "$prefix/lib/systemd/user/$daemon.service.d/sandbox.conf"
    done
else
    # One an earlier install left would keep the daemons from starting.
    for daemon in $daemons; do
        rm -f "$prefix/lib/systemd/user/$daemon.service.d/sandbox.conf"
    done
    echo "The daemons run without their extra sandbox on this system, which does not let a user service have its own mount namespace. On Ubuntu, allow it with the kernel.apparmor_restrict_unprivileged_userns setting or an AppArmor profile, then run this again."
fi
# porter-mountd and its socket. The socket is enabled for every user as a package does it: a link
# in sockets.target.wants beside the units (no `systemctl`, which would write under /etc or a home).
# The service has no sandbox drop-in: what it may not have is said in its header.
put 755 "$built/porter-mountd" "$libexec/porter-mountd"
put 644 "$dist/porter-mountd.service" "$prefix/lib/systemd/user/porter-mountd.service" /usr/libexec/quire "$libexec"
put 644 "$dist/porter-mountd.socket" "$prefix/lib/systemd/user/porter-mountd.socket"
link "../porter-mountd.socket" "$prefix/lib/systemd/user/sockets.target.wants/porter-mountd.socket"
for activation in "$dist"/dbus/*.service; do
    put 644 "$activation" "$prefix/share/dbus-1/services/$(basename "$activation")" /usr/libexec/quire "$libexec"
done

# --- Data ----------------------------------------------------------------------------------------
for provider in "$root"/providers/*.toml; do
    put 644 "$provider" "$prefix/share/porter/providers/$(basename "$provider")"
done
if [ -f "$dist/clients.toml" ]; then
    put 644 "$dist/clients.toml" "$prefix/share/porter/clients.toml"
fi
put 644 "$dist/inferd.settings.toml" "$prefix/share/quire/settings/inferd.settings.toml"
# The tailnet drop-in is data, never live: the Settings switch "Let my other computers use this
# computer's models" copies it into the person's own systemd user configuration, with their consent.
put 644 "$dist/inferd-tailnet.conf" "$prefix/share/porter/inferd-tailnet.conf"
put 644 "$dist/inferd.toml" "$prefix/share/doc/porter/examples/inferd.toml"
put 644 "$dist/inferd-cloud.conf" "$prefix/share/doc/porter/examples/inferd-cloud.conf"

# --- The caller tables ---------------------------------------------------------------------------
# accountd's and syncd's (callers.toml) and inferd's (inferd-callers.toml). Each is the machine
# owner's to edit once installed: a copy that differs is kept, and the shipped one is left beside
# it as <name>.dist for them to compare.
for table in callers.toml inferd-callers.toml; do
    callers=$sysconfdir/porter/$table
    if [ -f "$destdir$callers" ] && ! cmp -s "$dist/$table" "$destdir$callers"; then
        echo "kept $callers (it differs from the shipped one; the shipped one is $callers.dist)"
        put 644 "$dist/$table" "$callers.dist"
    else
        put 644 "$dist/$table" "$callers"
    fi
done

if [ "$changed" -eq 0 ]; then
    echo "nothing to change"
fi

# A fixed role (the daemons' own) is kept only by a unit root's files define, in one of the three
# folders systemd reads for every user (peephole's `Full` proof, used by porter-dbus callers). Anywhere else any program of
# the person's could change the unit, so the daemons run with an app's permissions.
if [ -z "$destdir" ]; then
    case "$prefix/lib/systemd/user" in
        /usr/lib/systemd/user | /usr/local/lib/systemd/user) ;;
        *)
            echo "Installed for you only. This is for development: the desktop's own services run with an app's permissions, because any program of yours could change units kept outside the system's folders. For everyday use, install system-wide: scripts/install.sh (with the default prefix, as root)."
            ;;
    esac
fi
