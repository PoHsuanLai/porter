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
# porter-http is the pure seam and reaches none of them; porter-proxy and porter-oauth reach tokio
# only through their named I/O feature (`io`). reqwest is named by one crate, porter-http-reqwest
# (see ONE CLIENT below), and reached through it by the families that send directly. porter-families reaches none with default features;
# each of its family features is checked on its own in FAMILY_FORBIDS below. porter-drive and
# porter-drive-fuse reach tokio (a drive fetches in the background) but never a bus or a client.
# porter-mountd, the mount helper outside syncd's sandbox, reaches no effect crate and none of
# porter's own: std, rustix and listenfd only (no network, keyring or account data).
EFFECTS="zbus zvariant tokio reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
RULES=(
  "porter-ids: $EFFECTS toml"
  "porter-capability: $EFFECTS toml"
  "porter-match: $EFFECTS toml"
  "porter-core: $EFFECTS toml"
  "porter-wire: $EFFECTS toml"
  "porter-sheet: $EFFECTS toml"
  "porter-fs: $EFFECTS"
  "porter-provider: $EFFECTS"
  "porter-secrets: $EFFECTS"
  "porter-sync: $EFFECTS"
  "porter-infer: $EFFECTS"
  "porter-bridge: $EFFECTS"
  "porter-router: $EFFECTS inferd"
  "porter-turns: zbus zvariant ureq oo7 keyring secret-service interprocess latchkey ds-settings inferd"
  "porter-service: $EFFECTS"
  "porter-store: zbus zvariant reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
  "porter-client: $EFFECTS"
  "porter-sheets: reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
  "porter-fake: $EFFECTS"
  "test-kit: $EFFECTS"
  "porter-http: $EFFECTS"
  "porter-proxy: $EFFECTS"
  "porter-oauth: $EFFECTS"
  "porter-discover: $EFFECTS"
  "porter-daemon: $EFFECTS"
  "porter-tailscale: $EFFECTS"
  "porter-tailnet: $EFFECTS"
  "dav-proto: $EFFECTS"
  "nextcloud-proto: $EFFECTS"
  "generic-proto: $EFFECTS"
  "porter-families: $EFFECTS"
  "porter-engines: $EFFECTS"
  "porter-engines-install: zbus zvariant ureq oo7 keyring secret-service interprocess latchkey ds-settings inferd"
  "storage-webdav: $EFFECTS"
  "storage-graph: $EFFECTS"
  "storage-gdrive: $EFFECTS"
  "porter-drive: zbus zvariant reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
  "porter-mountd: $EFFECTS fuser porter-ids porter-core porter-fs porter-http porter-secrets porter-service porter-client porter-sync porter-drive porter-drive-fuse"
  "porter-drive-fuse: zbus zvariant reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
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
#   oauth:              tokio, hyper, reqwest (porter-oauth's `io`, porter-http-reqwest)
#   api_key:            hyper, reqwest, tokio (porter-http-reqwest)
#   tailnet:            hyper, tokio (porter-tailscale's `io`)
#   nextcloud, agent_login: none
#   set:                every family a daemon maps (what oauth, generic, api_key and tailnet
#                       reach together), with the system resolver (porter-discover's `io`)
# Every feature in porter-families' Cargo.toml must have a row, so a new family cannot slip in
# unchecked; the direct porter-* edges of each feature are exact too (FAMILY_EDGES).
MS_FORBIDS="zbus zvariant reqwest ureq oo7 keyring secret-service interprocess latchkey ds-settings"
DIRECT_FORBIDS="zbus zvariant ureq oo7 keyring secret-service interprocess latchkey ds-settings"
declare -A FAMILY_FORBIDS=(
  [nextcloud]="$EFFECTS"
  [generic]="zbus zvariant hyper reqwest ureq oo7 keyring secret-service interprocess latchkey ds-settings"
  [oauth]="$DIRECT_FORBIDS"
  [api_key]="$DIRECT_FORBIDS"
  [agent_login]="$EFFECTS"
  [tailnet]="$MS_FORBIDS"
  [set]="$DIRECT_FORBIDS"
)
declare -A FAMILY_EDGES=(
  [nextcloud]="dav-proto nextcloud-proto porter-core porter-capability porter-sheet porter-ids porter-http porter-provider"
  [generic]="generic-proto porter-core porter-capability porter-sheet porter-ids porter-discover porter-http porter-provider porter-proxy"
  [oauth]="porter-core porter-capability porter-sheet porter-ids porter-http porter-http-reqwest porter-oauth porter-provider"
  [api_key]="porter-core porter-capability porter-sheet porter-ids porter-http porter-http-reqwest porter-provider"
  [agent_login]="porter-core porter-capability porter-sheet porter-ids porter-http porter-provider"
  [tailnet]="porter-core porter-capability porter-sheet porter-ids porter-http porter-provider porter-tailscale"
  [set]="dav-proto nextcloud-proto generic-proto porter-core porter-capability porter-sheet porter-ids porter-discover porter-http porter-http-reqwest porter-oauth porter-provider porter-proxy porter-tailscale"
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

# ONE CLIENT: reqwest is named by porter-http-reqwest and by no other crate of this workspace. Every
# other crate that sends over a direct connection takes `ReqwestHttp` from it.
named=$(grep -lE '^reqwest[ .=]' crates/*/Cargo.toml | grep -v '^crates/porter-http-reqwest/' || true)
if [ -n "$named" ]; then
  echo "LEAK: reqwest is named outside porter-http-reqwest: $named"
  fail=1
else
  echo "boundary holds: only porter-http-reqwest names reqwest"
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
  "crates/porter-ids/src/clock.rs"
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

# porter-core's and porter-ids' source is pure: the file writer is porter-fs, so no file, process or
# environment access is named anywhere in either crate (a dependency rule cannot see std's own
# modules).
for pure in porter-ids porter-core; do
  if grep -rnE 'std::(fs|process|env)\b' "crates/$pure/src"; then
    echo "LEAK: $pure's source reaches std's file, process or environment access"
    fail=1
  else
    echo "pure: $pure's source names no std::fs, std::process or std::env"
  fi
done

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

# DELETED HELPERS: hand-rolled code that a crate or one shared function replaced must not come
# back. Each rule names the code and where its one home is (a path prefix that is allowed to hold
# it, if any); the allowlist is otherwise empty, and every crate is covered.
ratchet() {
  local what="$1" pattern="$2" home="${3:-}" hits
  hits=$(grep -rniE --include='*.rs' -e "$pattern" crates | grep -v "^${home:-/nonexistent}" || true)
  if [ -n "$hits" ]; then
    echo "$hits"
    echo "LEAK: $what is back; use ${4:-the crate that replaced it}"
    fail=1
  else
    echo "deleted: $what is not in any crate"
  fi
}
ratchet "a hand-rolled percent encoder or decoder" 'fn percent_(en|de)code\b' "" "porter_ids::percent"
ratchet "hand-rolled civil-date arithmetic" '(days_from_civil|civil_from_days|719_468)' "" "chrono"
ratchet "a hand-written MD5 (its first round constant)" '0xd76aa478' "" "md-5"
ratchet "a second Windows-to-IANA zone table" 'Pacific Standard Time", *"America/Los_Angeles' "crates/windows-zones/" "windows_zones::iana"
ratchet "an XOAUTH2 format string" '(\\x01|\\u\{1\})auth=Bearer|auth=Bearer \{|"auth=Bearer' "crates/porter-core/src/token.rs" "porter_core::token"
ratchet "a hand-written scratch directory type" '^[[:space:]]*(pub(\([a-z]+\))? )?struct Scratch\b' "crates/test-kit/" "test_kit::Scratch"

# The allowed edges between our own crates: each crate's DIRECT normal and build path
# dependencies (all features), and nothing else. A dependency not listed is a leak; so is one
# the crate no longer has, so the table stays exact. Dev dependencies are outside it.
EDGES=(
  "porter-ids:"
  "porter-capability: porter-ids"
  "porter-match: porter-capability porter-ids"
  "porter-core: porter-ids porter-capability"
  "porter-wire: porter-core porter-match porter-capability"
  "porter-sheet: porter-core porter-tailscale porter-capability porter-ids"
  "porter-daemon:"
  "porter-fs:"
  "quickxorhash:"
  "windows-zones:"
  "keyring-chunked:"
  "porter-provider: porter-core porter-match porter-capability porter-sheet porter-ids"
  "porter-secrets: keyring-chunked porter-core porter-capability porter-ids porter-fs"
  "porter-sync: porter-capability porter-ids porter-http"
  "porter-infer: porter-core porter-match porter-capability porter-ids cua-action model-provider"
  "porter-service: porter-core porter-tailscale porter-match porter-capability porter-sheet porter-wire porter-ids porter-provider porter-secrets"
  "porter-store: porter-core porter-capability porter-fs"
  "porter-router: porter-core porter-tailnet porter-tailscale porter-match porter-capability porter-ids porter-engines porter-infer cua-action engine-supervisor model-catalog model-http model-openai-compat model-provider speech-provider vision-prep"
  "porter-turns: porter-core porter-match porter-capability porter-ids porter-infer porter-bridge porter-router cua-action cua-parse cua-session cua-vendors engine-supervisor model-extract model-http model-openai-compat model-provider vision-prep"
  "porter-dbus: porter-core porter-tailscale porter-match porter-wire porter-ids"
  "porter-bridge: porter-core porter-capability porter-match porter-ids porter-infer model-catalog model-openai-compat model-provider vision-prep"
  "porter-sheets: porter-core porter-capability porter-sheet porter-wire porter-dbus porter-provider porter-service"
  "porter-client: porter-bridge porter-core porter-tailnet porter-tailscale porter-match porter-capability porter-wire porter-ids porter-dbus porter-fs porter-infer porter-provider porter-proxy porter-secrets porter-service porter-turns model-http model-openai-compat model-provider model-wire"
  "porter-fake: porter-core porter-capability porter-sheet porter-wire porter-ids porter-provider porter-secrets porter-service"
  "test-kit:"
  "porter-fake-servers: dav-proto porter-core porter-ids porter-discover porter-fake porter-provider quickxorhash test-kit"
  "porter-http: porter-core porter-ids"
  "porter-http-reqwest: porter-ids porter-http"
  "porter-http-relay: porter-client porter-core porter-wire porter-ids porter-http"
  "porter-engines:"
  "porter-engines-install: porter-ids porter-engines porter-http porter-http-reqwest"
  "porter-proxy: porter-core porter-capability porter-ids"
  "porter-oauth: porter-core porter-ids porter-http porter-provider"
  "porter-discover: porter-core porter-capability porter-ids porter-http porter-provider"
  "porter-tailscale: porter-ids"
  "porter-tailnet: porter-ids porter-fs porter-tailscale"
  "dav-proto:"
  "nextcloud-proto: porter-core porter-capability porter-ids"
  "generic-proto: dav-proto porter-core porter-capability porter-ids porter-provider porter-sheet"
  "porter-families: dav-proto nextcloud-proto generic-proto porter-core porter-capability porter-sheet porter-ids porter-discover porter-http porter-http-reqwest porter-oauth porter-provider porter-proxy porter-tailscale"
  "accountd: ds-settings porter-core porter-match porter-capability porter-sheet porter-wire porter-ids porter-daemon porter-dbus porter-families porter-fs porter-provider porter-secrets porter-service porter-sheets porter-store porter-tailscale"
  "storage-webdav: dav-proto porter-capability porter-ids porter-http porter-sync"
  "storage-graph: porter-capability porter-ids porter-http porter-sync"
  "storage-gdrive: porter-capability porter-ids porter-http porter-sync"
  "porter-drive: porter-capability porter-ids porter-fs porter-sync quickxorhash"
  "porter-drive-fuse: porter-drive porter-mountd"
  "porter-mountd:"
  "syncd: dav-proto ds-settings porter-client porter-core porter-match porter-capability porter-sheet porter-wire porter-ids porter-daemon porter-dbus porter-drive porter-drive-fuse porter-families porter-fs porter-http porter-http-relay porter-provider porter-secrets porter-service porter-sheets porter-store porter-sync storage-gdrive storage-graph storage-webdav windows-zones"
  "porter-rig: fake-secret-service porter-client porter-core porter-match porter-capability porter-wire porter-ids porter-dbus porter-fake porter-fake-servers porter-infer test-kit"
  "fake-secret-service:"
  "syncd-fakes: porter-dbus"
  "inferd: ds-settings porter-bridge porter-engines porter-families porter-secrets porter-service porter-sheets porter-store porter-router porter-turns porter-core porter-match porter-capability porter-sheet porter-wire porter-ids porter-daemon porter-dbus porter-discover porter-fs porter-http porter-http-reqwest porter-infer porter-provider porter-tailnet porter-tailscale cua-action cua-parse cua-session cua-vendors engine-supervisor model-catalog model-extract model-http model-openai-compat model-provider model-replay model-wire speech-host-client speech-provider vision-prep"
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
for crate in accountd inferd syncd porter-secrets; do
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

# porter-dbus's test-only caller fixtures (feature `test-proc-root`, which turns on peephole's
# `fake`) are never in a daemon's normal dependency tree.
for crate in accountd inferd syncd; do
  if cargo tree -p "$crate" -e normal --prefix none -f '{p} {f}' 2>/dev/null | grep -E '^(porter-dbus|peephole) ' | grep -Eq 'test-proc-root|fake'; then
    echo "LEAK: $crate's default build enables porter-dbus test-proc-root or peephole fake"
    fail=1
  else
    echo "test-only: $crate's default build has no caller fixtures"
  fi
done

# Every workspace member has a row above, so a new crate cannot slip in unchecked.
for member in $(sed -n 's#^  "crates/\(.*\)",$#\1#p' Cargo.toml); do
  printf '%s\n' "${EDGES[@]}" | grep -q "^$member:" || { echo "ERROR: $member has no row in EDGES"; fail=1; }
done

# porter-fake is built over porter-core, porter-provider, porter-secrets and porter-service, so none
# of them (nor anything below them) may take it, even as a dev dependency: a dev-dependency
# cycle makes the crate's own tests compile it twice. Their tests use `test-kit`, which knows
# nothing of porter; tests that need the fakes live in porter-fake's own tests.
for below in porter-core porter-provider porter-secrets porter-service; do
  if cargo tree -p "$below" -e normal,build,dev --prefix none 2>/dev/null | grep -qx 'porter-fake v.*'; then
    echo "LEAK: $below reaches porter-fake, which depends on it (a dev-dependency cycle)"
    fail=1
  else
    echo "no cycle: $below does not reach porter-fake"
  fi
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

# LIBRARY ERRORS CAN GROW: every `pub enum` whose name ends in `Error`, in a library crate, is
# `#[non_exhaustive]`, so a variant added later breaks no caller's `match`. A library crate is
# one a shipped program links: the programs are the binaries dist/ names, the crates they reach
# through normal and build dependencies are the shipped ones, and of those a daemon (a crate
# with a `daemon` module) holds its own errors for itself. Test tooling is never reached from a
# shipped program, so it is outside the rule by what it is.
shipped_programs=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | jq -r '.packages[] | .name as $p | .targets[] | select(.kind | index("bin")) | "\($p) \(.name)"' \
  | while read -r package program; do grep -rqF -- "$program" dist && echo "$package"; done | sort -u)
shipped=$(for package in $shipped_programs; do
  cargo tree -p "$package" -e normal,build --prefix none 2>/dev/null | grep -E '\(/' | awk '{print $1}'
done | sort -u)
grow_leaks=""
for crate in $shipped; do
  [ -f "crates/$crate/src/lib.rs" ] && [ ! -f "crates/$crate/src/daemon.rs" ] || continue
  found=$(find "crates/$crate/src" -name '*.rs' ! -name 'tests.rs' ! -name '*_tests.rs' ! -name 'testing.rs' \
    ! -name 'testkit.rs' ! -path '*/tests/*' ! -path '*/testkit/*' ! -path '*/testing/*' -print0 \
    | xargs -0 awk '
      FNR == 1 { exhaustive = 1 }
      /^[[:space:]]*#\[non_exhaustive\]/ { exhaustive = 0; next }
      /^[[:space:]]*pub enum [A-Za-z0-9_]*Error([^A-Za-z0-9_]|$)/ {
        if (exhaustive) print FILENAME ":" FNR ": " $0
        exhaustive = 1
        next
      }
      /^[[:space:]]*(pub(\([a-z]+\))? )?(fn|struct|enum|impl|trait|mod|use|const|static|type|union|extern|unsafe|async)[[:space:]]/ { exhaustive = 1 }
      /^}/ { exhaustive = 1 }')
  [ -n "$found" ] && grow_leaks="$grow_leaks$found
"
done
if [ -n "$grow_leaks" ]; then
  printf '%s' "$grow_leaks"
  echo "LEAK: a public error enum of a library crate is not #[non_exhaustive]"
  fail=1
else
  echo "grows: every public *Error enum of the libraries is #[non_exhaustive] ($(echo $shipped | wc -w) shipped crates looked at)"
fi

# The portable cores build without the desktop and reach no D-Bus; the accounts-only client
# reaches no inference either (quire design/36).
./scripts/check-portable.sh || fail=1

exit "$fail"
