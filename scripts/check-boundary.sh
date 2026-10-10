#!/usr/bin/env bash
# Crate boundaries, mechanically enforced (ARCHITECTURE.md section 1 is the table; this is its
# mechanical form).
#
# `cargo tree -i <dep>` exits 101 when the dependency is absent, which is precisely the state
# we want. Checking the exit status would therefore fail whenever the boundary holds, so we
# check for OUTPUT instead: any line naming the dependency is a leak.
set -uo pipefail
cd "$(dirname "$0")/.."

# RULES: what a crate reaches through ANY path (transitive, default features). The pure crates
# never reach a bus, a runtime, an HTTP client or a keyring; porter-client reaches zbus only
# through its `dbus` feature; porter-dbus reaches tokio only through zbus's `tokio` feature.
# porter-http, porter-proxy and porter-oauth reach tokio (and porter-http hyper) only through
# their named I/O feature (`hyper`, `io`). porter-families reaches none with default features;
# each of its family features is checked on its own in FAMILY_FORBIDS below.
EFFECTS="zbus zvariant tokio reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
RULES=(
  "porter-core: $EFFECTS toml"
  "porter-fs: $EFFECTS"
  "prov: $EFFECTS"
  "porter-provider: $EFFECTS"
  "porter-secrets: $EFFECTS"
  "porter-sync: $EFFECTS"
  "porter-infer: $EFFECTS"
  "porter-bridge: $EFFECTS"
  "porter-router: $EFFECTS inferd"
  "porter-turns: zbus zvariant reqwest ureq oo7 keyring secret-service interprocess latchkey ds-settings inferd"
  "porter-service: $EFFECTS"
  "porter-client: $EFFECTS"
  "porter-fake: $EFFECTS"
  "porter-http: $EFFECTS"
  "porter-proxy: $EFFECTS"
  "porter-oauth: $EFFECTS"
  "porter-discover: $EFFECTS"
  "porter-daemon: $EFFECTS"
  "porter-tailscale: $EFFECTS"
  "porter-tailnet: $EFFECTS"
  "porter-dav: $EFFECTS"
  "porter-families: $EFFECTS"
  "storage-webdav: $EFFECTS"
  "storage-graph: $EFFECTS"
  "storage-gdrive: $EFFECTS"
  "porter-dbus: reqwest hyper ureq oo7 keyring secret-service ds-settings"
)
fail=0

for rule in "${RULES[@]}"; do
  crate="${rule%%:*}"
  read -r -a forbidden <<<"${rule#*:}"
  # A crate that cargo cannot find would make every check below pass vacuously.
  if ! cargo tree -p "$crate" --depth 0 >/dev/null 2>&1; then
    echo "ERROR: cargo tree cannot resolve $crate; the boundary was not checked"
    fail=1
    continue
  fi
  leaked=0
  for dep in "${forbidden[@]}"; do
    if cargo tree -p "$crate" -i "$dep" -e normal,build 2>/dev/null | grep -q .; then
      echo "LEAK: $crate depends on $dep"
      cargo tree -p "$crate" -i "$dep" -e normal,build 2>/dev/null | head -20
      leaked=1
      fail=1
    fi
  done
  if [ "$leaked" -eq 0 ]; then
    echo "boundary holds: $crate reaches none of ${forbidden[*]}"
  fi
done

# FAMILY FEATURES: porter-families with each of its family features on, one at a time, and no
# other. Its default build is checked by RULES above (it reaches none of EFFECTS). A family
# reaches only what it really uses, so each feature forbids EFFECTS minus what it uses today:
#   generic:            tokio (porter-proxy's `io`, for the relay's login)
#   microsoft, google:  tokio, hyper (porter-oauth's `io`, porter-http's `hyper`)
#   api_key:            hyper, tokio (porter-http's `hyper`)
#   tailnet:            hyper, tokio (porter-tailscale's `io`)
#   nextcloud, agent_login, openrouter: none
# Every feature in porter-families' Cargo.toml must have a row, so a new family cannot slip in
# unchecked; the direct porter-* edges of each feature are exact too (FAMILY_EDGES).
MS_FORBIDS="zbus zvariant reqwest ureq oo7 keyring secret-service interprocess latchkey ds-settings"
declare -A FAMILY_FORBIDS=(
  [nextcloud]="$EFFECTS"
  [generic]="zbus zvariant hyper reqwest ureq oo7 keyring secret-service interprocess latchkey ds-settings"
  [microsoft]="$MS_FORBIDS"
  [google]="$MS_FORBIDS"
  [api_key]="$MS_FORBIDS"
  [agent_login]="$EFFECTS"
  [openrouter]="$EFFECTS"
  [tailnet]="$MS_FORBIDS"
)
declare -A FAMILY_EDGES=(
  [nextcloud]="porter-core porter-dav porter-discover porter-http porter-provider"
  [generic]="porter-core porter-dav porter-discover porter-http porter-provider porter-proxy"
  [microsoft]="porter-core porter-discover porter-http porter-oauth porter-provider"
  [google]="porter-core porter-http porter-oauth porter-provider"
  [api_key]="porter-core porter-http porter-provider"
  [agent_login]="porter-core porter-http porter-provider"
  [openrouter]="porter-core porter-http porter-oauth porter-provider"
  [tailnet]="porter-core porter-http porter-provider porter-tailscale"
)
family_features=$(awk '/^\[features\]/{f=1;next} /^\[/{f=0} f && /^[A-Za-z0-9_-]+ *=/{sub(/ *=.*/,""); if ($0!="default") print}' crates/porter-families/Cargo.toml)
for feature in $family_features; do
  if [ -z "${FAMILY_FORBIDS[$feature]+set}" ] || [ -z "${FAMILY_EDGES[$feature]+set}" ]; then
    echo "ERROR: porter-families feature $feature has no row in FAMILY_FORBIDS or FAMILY_EDGES; its boundary was not checked"
    fail=1
    continue
  fi
  read -r -a forbidden <<<"${FAMILY_FORBIDS[$feature]}"
  leaked=0
  for dep in "${forbidden[@]}"; do
    if cargo tree -p porter-families --no-default-features --features "$feature" -i "$dep" -e normal,build 2>/dev/null | grep -q .; then
      echo "LEAK: porter-families[$feature] depends on $dep"
      cargo tree -p porter-families --no-default-features --features "$feature" -i "$dep" -e normal,build 2>/dev/null | head -20
      leaked=1
      fail=1
    fi
  done
  if [ "$leaked" -eq 0 ]; then
    echo "boundary holds: porter-families[$feature] reaches none of ${forbidden[*]}"
  fi
  found=$(cargo tree -p porter-families --no-default-features --features "$feature" --depth 1 -e normal,build --prefix none 2>/dev/null \
    | grep -E '\((/|https://github.com/PoHsuanLai/(stoker|quire))' | awk '{print $1}' | grep -vx porter-families | sort -u | tr '\n' ' ')
  want=$(printf '%s\n' ${FAMILY_EDGES[$feature]} | sort -u | tr '\n' ' ')
  if [ "$found" != "$want" ]; then
    echo "EDGE: porter-families[$feature] depends on [${found% }], the table allows [${want% }]"
    fail=1
  else
    echo "edges hold: porter-families[$feature] depends on [${found% }]"
  fi
done

# IN-PROCESS FEATURE: porter-client's feature `in-process` is the only way the crate reaches the
# account service, its secrets and the provider catalogue. A bus-only consumer (`dbus` with no
# default features) reaches none of them.
in_process_leaked=0
for dep in porter-service porter-secrets porter-provider; do
  if cargo tree -p porter-client --no-default-features --features dbus -i "$dep" -e normal,build 2>/dev/null | grep -q .; then
    echo "LEAK: porter-client[dbus] without in-process depends on $dep"
    cargo tree -p porter-client --no-default-features --features dbus -i "$dep" -e normal,build 2>/dev/null | head -20
    in_process_leaked=1
    fail=1
  fi
done
if [ "$in_process_leaked" -eq 0 ]; then
  echo "boundary holds: porter-client[dbus] without in-process reaches none of porter-service, porter-secrets, porter-provider"
fi

# STREAM FEATURE: porter-http's `stream` (HTTP/1.1 over a byte stream, porter-core's ByteStream)
# adds no dependency, so the crate with that feature alone reaches none of EFFECTS.
stream_leaked=0
for dep in $EFFECTS; do
  if cargo tree -p porter-http --no-default-features --features stream -i "$dep" -e normal,build 2>/dev/null | grep -q .; then
    echo "LEAK: porter-http[stream] depends on $dep"
    cargo tree -p porter-http --no-default-features --features stream -i "$dep" -e normal,build 2>/dev/null | head -20
    stream_leaked=1
    fail=1
  fi
done
if [ "$stream_leaked" -eq 0 ]; then
  echo "boundary holds: porter-http[stream] reaches none of $EFFECTS"
fi

# LENDING FEATURE: porter-client reaches porter-fs (the writer of the tailnet drop-in) only
# through `lending` or `in-process` (porter-secrets uses it); a bus-only consumer reaches it not.
if cargo tree -p porter-client --no-default-features --features dbus -i porter-fs -e normal,build 2>/dev/null | grep -q .; then
  echo "LEAK: porter-client[dbus] without lending or in-process depends on porter-fs"
  fail=1
else
  echo "boundary holds: porter-client[dbus] without lending or in-process reaches no porter-fs"
fi

# PURE FILES: source that parses what an edge read and never reads it itself. A dependency rule
# cannot see std, so these are checked by name: no file, process, socket or environment access.
STD_EFFECTS='std::(fs|io|env|process|net|os)\b|\b(File|Command|TcpStream|UnixStream)::'
PURE_FILES=(
  "crates/porter-core/src/identity.rs"
  "crates/porter-core/src/clock.rs"
)
for file in "${PURE_FILES[@]}"; do
  if [ ! -f "$file" ]; then
    echo "ERROR: $file is missing; its purity was not checked"
    fail=1
  elif grep -nE "$STD_EFFECTS" "$file"; then
    echo "LEAK: $file reaches std's effects"
    fail=1
  else
    echo "pure: $file reaches none of std's effects"
  fi
done

# porter-core's source is pure: its file writer is porter-fs, so no file, process or environment
# access is named anywhere in the crate (a dependency rule cannot see std's own modules).
if grep -rnE 'std::(fs|process|env)\b' crates/porter-core/src; then
  echo "LEAK: porter-core's source reaches std's file, process or environment access"
  fail=1
else
  echo "pure: porter-core's source names no std::fs, std::process or std::env"
fi

# The three daemons' libraries read the environment in no function but `from_env` (the daemon's
# `Config::from_env` and the directory helpers it calls), so a program that runs a daemon inside
# itself gives it a `Config` and nothing else. The binaries (main.rs) and the test-only files are
# outside the rule; `std::env::temp_dir` is a scratch name in tests, not a read of a variable.
# The awk remembers the last `fn` it saw, which is the function the next line is in.
env_reads=$(find crates/accountd/src crates/syncd/src crates/inferd/src -name '*.rs' \
  ! -name main.rs ! -name tests.rs ! -name testing.rs ! -name testkit.rs ! -path '*/tests/*' -print0 \
  | xargs -0 awk '
    FNR == 1 { current = "" }
    /^[[:space:]]*(pub(\([a-z]+\))? )?(async )?(unsafe )?fn [A-Za-z_0-9]+/ {
      match($0, /fn [A-Za-z_0-9]+/)
      current = substr($0, RSTART + 3, RLENGTH - 3)
    }
    /env::(var|var_os|vars|vars_os|args|args_os|current_dir|home_dir)\>/ \
      && current != "from_env" && $0 !~ /^[[:space:]]*\/\// {
      print FILENAME ":" FNR ": " $0
    }')
if [ -n "$env_reads" ]; then
  echo "$env_reads"
  echo "LEAK: a daemon library reads the environment outside a from_env function"
  fail=1
else
  echo "pure: accountd, syncd and inferd read the environment only in from_env functions"
fi

# The inference libraries (porter-router, porter-turns) and porter-daemon read no environment
# variable at all: what a daemon or an app knows is passed in (porter-daemon takes the lookup). Their test helpers are outside the
# rule, as above.
lib_env_reads=$(find crates/porter-router/src crates/porter-turns/src crates/porter-daemon/src -name '*.rs' ! -name tests.rs ! -name testkit.rs ! -path '*/testkit/*' -print0 \
  | xargs -0 grep -nE 'env::(var|var_os|vars|vars_os|args|args_os|current_dir|home_dir)\b' | grep -vE '^[^:]*:[0-9]+:[[:space:]]*//' || true)
if [ -n "$lib_env_reads" ]; then
  echo "$lib_env_reads"
  echo "LEAK: an inference library reads the environment"
  fail=1
else
  echo "pure: the inference libraries and porter-daemon read no environment variable"
fi

# The allowed edges between our own crates: each crate's DIRECT normal and build path
# dependencies (all features), and nothing else. A dependency not listed is a leak; so is one
# the crate no longer has, so the table stays exact. Dev dependencies are outside it.
EDGES=(
  "porter-core:"
  "porter-daemon:"
  "porter-fs:"
  "prov: porter-core"
  "porter-provider: porter-core"
  "porter-secrets: porter-core porter-fs"
  "porter-sync: porter-core"
  "porter-infer: porter-core cua-action"
  "porter-service: porter-core porter-provider porter-secrets"
  "porter-router: porter-core porter-infer cua-action engine-supervisor model-catalog model-http model-openai-compat model-provider speech-provider vision-prep"
  "porter-turns: porter-core porter-infer porter-bridge porter-router cua-action cua-parse cua-session cua-vendors engine-supervisor model-extract model-http model-openai-compat model-provider vision-prep"
  "porter-dbus: porter-core"
  "porter-bridge: porter-core porter-infer model-catalog model-openai-compat model-provider vision-prep"
  "porter-client: porter-bridge porter-core porter-dbus porter-fs porter-infer porter-provider porter-secrets porter-service porter-turns model-http model-openai-compat model-provider model-wire"
  "porter-fake: porter-core porter-infer porter-provider porter-secrets porter-service"
  "porter-fake-servers: porter-core porter-discover porter-fake porter-provider"
  "porter-http: porter-core"
  "porter-proxy: porter-core"
  "porter-oauth: porter-core porter-http porter-provider"
  "porter-discover: porter-core porter-http porter-provider"
  "porter-tailscale: porter-core"
  "porter-tailnet: porter-core porter-fs porter-tailscale"
  "porter-dav: porter-core porter-http"
  "porter-families: porter-core porter-dav porter-discover porter-http porter-oauth porter-provider porter-proxy porter-tailscale"
  "accountd: ds-settings porter-core porter-daemon porter-dbusporter-discover porter-families porter-fs porter-http porter-provider porter-proxy porter-secrets porter-service porter-tailscale"
  "storage-webdav: porter-core porter-dav porter-http porter-sync"
  "storage-graph: porter-core porter-http porter-sync storage-webdav"
  "storage-gdrive: porter-core porter-http porter-sync storage-webdav"
  "syncd: porter-client porter-core porter-daemon porter-dav porter-dbusporter-fs porter-http porter-sync storage-gdrive storage-graph storage-webdav"
  "porter-rig: porter-client porter-core porter-dbus porter-fake porter-fake-servers porter-infer"
  "inferd: ds-settings porter-bridge porter-client porter-router porter-turns porter-core porter-daemon porter-dbus porter-discover porter-fs porter-http porter-infer porter-provider porter-tailnet porter-tailscale cua-action cua-parse cua-session cua-vendors engine-supervisor model-catalog model-extract model-http model-openai-compat model-provider model-replay model-wire speech-host-client speech-provider vision-prep"
)
for edge in "${EDGES[@]}"; do
  crate="${edge%%:*}"
  read -r -a allowed <<<"${edge#*:}"
  found=$(cargo tree -p "$crate" --depth 1 -e normal,build --prefix none --all-features 2>/dev/null \
    | grep -E '\((/|https://github.com/PoHsuanLai/(stoker|quire))' | awk '{print $1}' | grep -vx "$crate" | sort -u | tr '\n' ' ')
  want=$(printf '%s\n' "${allowed[@]}" | grep . | sort -u | tr '\n' ' ')
  if [ "$found" != "$want" ]; then
    echo "EDGE: $crate depends on [${found% }], the table allows [${want% }]"
    fail=1
  else
    echo "edges hold: $crate depends on [${found% }]"
  fi
done

# accountd's test-only knob (`ACCOUNTD_PROC_ROOT`, feature `test-proc-root`) is never on in a
# default build: the features cargo resolves for accountd without any flag must not name it.
if cargo tree -p accountd --depth 0 -f '{p} {f}' 2>/dev/null | grep -q 'test-proc-root'; then
  echo "LEAK: accountd enables test-proc-root by default"
  fail=1
else
  echo "test-only: accountd's default features do not include test-proc-root"
fi

# accountd's file key store (`ACCOUNTD_KEYS=file:`, feature `test-keys`, porter-secrets'
# `FileSecrets`) keeps credentials as plain text: neither crate's default build, nor accountd's
# whole normal dependency tree, may have the feature on.
for crate in accountd porter-secrets; do
  if cargo tree -p "$crate" -e normal --prefix none -f '{p} {f}' 2>/dev/null | grep -q 'test-keys'; then
    echo "LEAK: $crate's default build enables test-keys"
    fail=1
  else
    echo "test-only: $crate's default build does not include test-keys"
  fi
done

# syncd's test-only knob (`SYNCD_PROC_ROOT`, feature `test-proc-root`), the same rule.
if cargo tree -p syncd --depth 0 -f '{p} {f}' 2>/dev/null | grep -q 'test-proc-root'; then
  echo "LEAK: syncd enables test-proc-root by default"
  fail=1
else
  echo "test-only: syncd's default features do not include test-proc-root"
fi

# Every workspace member has a row above, so a new crate cannot slip in unchecked.
for member in $(sed -n 's#^  "crates/\(.*\)",$#\1#p' Cargo.toml); do
  printf '%s\n' "${EDGES[@]}" | grep -q "^$member:" || { echo "ERROR: $member has no row in EDGES"; fail=1; }
done

# A test-only crate is never a dependency (normal, build or dev) of another crate of ours:
# `cargo tree -i` prints the crate itself on its first line and each dependent below it.
for testonly in porter-fake-servers; do
  dependents=$(cargo tree -p "$testonly" -i "$testonly" -e normal,build,dev --prefix none 2>/dev/null \
    | grep '(/' | awk '{print $1}' | grep -vx "$testonly" | tr '\n' ' ')
  if [ -n "$dependents" ]; then
    echo "LEAK: $testonly is a dependency of [${dependents% }]"
    fail=1
  else
    echo "test-only: nothing depends on $testonly"
  fi
done

# porter-rig is test tooling (the rig a jailed scenario drives porter with): no other crate of
# ours names it, not even as a dev dependency, it is never published, and nothing in dist/ (the
# units, the packaging) mentions it, so it is never installed.
for manifest in crates/*/Cargo.toml; do
  [ "$manifest" = "crates/porter-rig/Cargo.toml" ] && continue
  if grep -q 'porter-rig' "$manifest"; then
    echo "LEAK: $manifest names porter-rig"
    fail=1
  fi
done
if ! grep -q '^publish.workspace = true' crates/porter-rig/Cargo.toml \
  || ! grep -q '^publish = false' Cargo.toml; then
  echo "LEAK: porter-rig is publishable"
  fail=1
fi
if grep -rq 'porter-rig' dist; then
  echo "LEAK: dist/ names porter-rig"
  fail=1
else
  echo "test-only: nothing depends on porter-rig, and dist/ does not name it"
fi

# The portable cores build without the desktop and reach no D-Bus; the accounts-only client
# reaches no inference either (quire design/36).
./scripts/check-portable.sh || fail=1

exit "$fail"
