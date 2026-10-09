#!/bin/sh
# Install porter: the three daemons, their systemd user units and D-Bus activation files, the
# provider files and the caller table.
#
#   scripts/install.sh [--prefix DIR] [--destdir DIR] [--sysconfdir DIR] [--bin-dir DIR] [--no-build]
#
# --prefix      where the software goes (default /usr; the environment variable PREFIX also sets it)
# --destdir     a staging root put in front of every path, for packagers and for trying the install
#               (default none; the environment variable DESTDIR also sets it)
# --sysconfdir  where the caller table goes (default /etc)
# --bin-dir     install the already built daemons found in DIR instead of building them
# --no-build    install from target/release (or $CARGO_TARGET_DIR/release) without building first
#
# What goes where (DESTDIR, when given, is in front of all of it):
#   PREFIX/libexec/quire/{accountd,syncd,inferd}             the daemons
#   PREFIX/lib/systemd/user/{accountd,syncd,inferd}.service  the user units
#   PREFIX/share/dbus-1/services/org.quire.*.service         D-Bus activation
#   PREFIX/share/porter/providers/*.toml                     the provider files
#   PREFIX/share/porter/clients.toml                         only when dist/clients.toml exists
#   PREFIX/share/porter/inferd-tailnet.conf                  the tailnet drop-in, as data: the Settings switch
#                                                            installs it for the person; never installed live
#   PREFIX/share/quire/settings/inferd.settings.toml         the AI settings page schema
#   PREFIX/share/doc/porter/examples/                        inferd.toml and inferd-cloud.conf: samples
#                                                            for the person to copy; never installed live
#   SYSCONFDIR/porter/callers.toml                           the caller table (kept if the person changed it)
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

mode_of() {
    stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"
}

# --- The daemons ---------------------------------------------------------------------------------
daemons="accountd syncd inferd"
if [ -n "$bin_dir" ]; then
    built=$bin_dir
else
    if [ "$build" = yes ]; then
        (cd "$root" && cargo build --release -p accountd -p syncd -p inferd)
    fi
    built=${CARGO_TARGET_DIR:-$root/target}/release
fi
for daemon in $daemons; do
    if [ ! -x "$built/$daemon" ]; then
        echo "install.sh: $built/$daemon is not there; build first or point --bin-dir at the daemons" >&2
        exit 1
    fi
done

# The files in dist/ name /usr/libexec/quire; a different prefix changes that one path.
libexec=$prefix/libexec/quire
for daemon in $daemons; do
    put 755 "$built/$daemon" "$libexec/$daemon"
    put 644 "$dist/$daemon.service" "$prefix/lib/systemd/user/$daemon.service" /usr/libexec/quire "$libexec"
done
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

# --- The caller table ----------------------------------------------------------------------------
# It is the person's to edit once installed: a copy that differs is kept, and the shipped one is
# left beside it as callers.toml.dist for them to compare.
callers=$sysconfdir/porter/callers.toml
if [ -f "$destdir$callers" ] && ! cmp -s "$dist/callers.toml" "$destdir$callers"; then
    echo "kept $callers (it differs from the shipped one; the shipped one is $callers.dist)"
    put 644 "$dist/callers.toml" "$callers.dist"
else
    put 644 "$dist/callers.toml" "$callers"
fi

if [ "$changed" -eq 0 ]; then
    echo "nothing to change"
fi
