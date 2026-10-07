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
# their named I/O feature (`hyper`, `io`), and porter-families through no feature of its own.
EFFECTS="zbus zvariant tokio reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
RULES=(
  "porter-core: $EFFECTS toml"
  "prov: $EFFECTS"
  "porter-provider: $EFFECTS"
  "porter-secrets: $EFFECTS"
  "porter-sync: $EFFECTS"
  "porter-infer: $EFFECTS"
  "porter-bridge: $EFFECTS"
  "porter-service: $EFFECTS"
  "porter-client: $EFFECTS"
  "porter-fake: $EFFECTS"
  "porter-http: $EFFECTS"
  "porter-proxy: $EFFECTS"
  "porter-oauth: $EFFECTS"
  "porter-discover: $EFFECTS"
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

# PURE FILES: source that parses what an edge read and never reads it itself. A dependency rule
# cannot see std, so these are checked by name: no file, process, socket or environment access.
STD_EFFECTS='std::(fs|io|env|process|net|os)\b|\b(File|Command|TcpStream|UnixStream)::'
PURE_FILES=(
  "crates/porter-core/src/identity.rs"
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

# The allowed edges between our own crates: each crate's DIRECT normal and build path
# dependencies (all features), and nothing else. A dependency not listed is a leak; so is one
# the crate no longer has, so the table stays exact. Dev dependencies are outside it.
EDGES=(
  "porter-core:"
  "prov: porter-core"
  "porter-provider: porter-core"
  "porter-secrets: porter-core"
  "porter-sync: porter-core"
  "porter-infer: porter-core cua-action"
  "porter-service: porter-core porter-provider porter-secrets"
  "porter-dbus: porter-core"
  "porter-bridge: porter-core porter-infer model-catalog model-openai-compat model-provider vision-prep"
  "porter-client: porter-bridge porter-core porter-dbus porter-infer porter-provider porter-secrets porter-service model-http model-openai-compat model-provider model-wire"
  "porter-fake: porter-core porter-infer porter-provider porter-secrets porter-service"
  "porter-fake-servers: porter-core porter-discover porter-fake porter-provider"
  "porter-http: porter-core"
  "porter-proxy: porter-core"
  "porter-oauth: porter-core porter-http porter-provider"
  "porter-discover: porter-core porter-http porter-provider"
  "porter-dav: porter-core porter-http"
  "porter-families: porter-core porter-dav porter-discover porter-http porter-oauth porter-provider"
  "accountd: ds-settings porter-core porter-dbus porter-discover porter-families porter-http porter-provider porter-proxy porter-secrets porter-service"
  "storage-webdav: porter-core porter-dav porter-http porter-sync"
  "storage-graph: porter-core porter-http porter-sync storage-webdav"
  "storage-gdrive: porter-core porter-http porter-sync storage-webdav"
  "syncd: porter-client porter-core porter-dav porter-dbus porter-http porter-sync storage-gdrive storage-graph storage-webdav"
  "porter-rig: porter-client porter-core porter-dbus porter-fake porter-fake-servers porter-infer"
  "inferd: ds-settings porter-bridge porter-core porter-dbus porter-discover porter-http porter-infer porter-provider cua-action cua-parse cua-session cua-vendors engine-supervisor model-catalog model-extract model-http model-openai-compat model-provider model-replay model-wire speech-host-client speech-provider vision-prep"
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
