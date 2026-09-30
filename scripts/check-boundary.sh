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
EFFECTS="zbus zvariant tokio reqwest hyper ureq oo7 keyring secret-service interprocess latchkey"
RULES=(
  "porter-core: $EFFECTS toml"
  "porter-provider: $EFFECTS"
  "porter-secrets: $EFFECTS"
  "porter-sync: $EFFECTS"
  "porter-infer: $EFFECTS"
  "porter-service: $EFFECTS"
  "porter-client: $EFFECTS"
  "porter-fake: $EFFECTS"
  "porter-dbus: reqwest hyper ureq oo7 keyring secret-service"
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

# The allowed edges between our own crates: each crate's DIRECT normal and build path
# dependencies (all features), and nothing else. A dependency not listed is a leak; so is one
# the crate no longer has, so the table stays exact. Dev dependencies are outside it.
EDGES=(
  "porter-core:"
  "porter-provider: porter-core"
  "porter-secrets: porter-core"
  "porter-sync: porter-core"
  "porter-infer: porter-core"
  "porter-service: porter-core porter-provider porter-secrets"
  "porter-dbus: porter-core"
  "porter-client: porter-core porter-dbus porter-infer porter-provider porter-secrets porter-service"
  "porter-fake: porter-core porter-infer porter-provider porter-secrets porter-service"
  "accountd: porter-core porter-dbus porter-provider porter-secrets porter-service"
  "syncd: porter-dbus porter-sync"
  "inferd: porter-core porter-dbus porter-infer"
)
for edge in "${EDGES[@]}"; do
  crate="${edge%%:*}"
  read -r -a allowed <<<"${edge#*:}"
  found=$(cargo tree -p "$crate" --depth 1 -e normal,build --prefix none --all-features 2>/dev/null \
    | grep '(/' | awk '{print $1}' | grep -vx "$crate" | sort -u | tr '\n' ' ')
  want=$(printf '%s\n' "${allowed[@]}" | grep . | sort -u | tr '\n' ' ')
  if [ "$found" != "$want" ]; then
    echo "EDGE: $crate depends on [${found% }], the table allows [${want% }]"
    fail=1
  else
    echo "edges hold: $crate depends on [${found% }]"
  fi
done

# Every workspace member has a row above, so a new crate cannot slip in unchecked.
for member in $(sed -n 's#^  "crates/\(.*\)",$#\1#p' Cargo.toml); do
  printf '%s\n' "${EDGES[@]}" | grep -q "^$member:" || { echo "ERROR: $member has no row in EDGES"; fail=1; }
done

exit "$fail"
