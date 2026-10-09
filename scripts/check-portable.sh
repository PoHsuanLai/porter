#!/usr/bin/env bash
# The portable core (quire design/36 section 1, rule 3 and section 4): everything except the
# desktop itself builds without the desktop. Each core below must `cargo check` with the
# features named, and its normal dependency tree must reach no D-Bus (zbus, zvariant). The two
# porter-client builds an accounts-only consumer uses (`default-features = false`, with and
# without `socket`) must also reach no inference: no porter-infer, and so no stoker.
#
# Called from check-boundary.sh, which the lane gate runs. Output says each check and each
# tree; the exit status is the verdict.
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0

# What no portable core reaches.
BUS='zbus|zvariant'
# What the accounts-only builds also never reach: inference, and stoker through it (cua-action is
# stoker's crate, by sibling path).
INFER='porter-infer|cua-action'

# check <label> <crate> <forbidden regex> [cargo feature flags...]
check() {
  local label=$1 crate=$2 forbidden=$3
  shift 3
  if cargo check -p "$crate" "$@" >/dev/null 2>&1; then
    echo "check ok: $label"
  else
    echo "FAIL check: $label does not build"
    fail=1
  fi
  local tree
  if ! tree=$(cargo tree -p "$crate" "$@" -e normal --prefix none 2>/dev/null); then
    echo "FAIL tree: cargo tree cannot resolve $label"
    fail=1
    return
  fi
  local leaked
  # `name vX.Y.Z` is the first two fields; a path dependency also names its directory.
  leaked=$(awk '{print $1}' <<<"$tree" | grep -xE "$forbidden" | sort -u | tr '\n' ' ')
  if grep -q '/stoker/' <<<"$tree" && [[ "$forbidden" == *porter-infer* ]]; then
    leaked="$leaked(a stoker path) "
  fi
  if [ -n "$leaked" ]; then
    echo "LEAK: $label reaches ${leaked% }"
    fail=1
  else
    echo "tree ok: $label reaches none of ${forbidden//|/ }"
  fi
}

check "porter-client --no-default-features" porter-client "$BUS|$INFER" --no-default-features
check "porter-client --no-default-features --features socket" porter-client "$BUS|$INFER" \
  --no-default-features --features socket
check "porter-core" porter-core "$BUS"
check "porter-provider" porter-provider "$BUS"
check "porter-secrets --no-default-features" porter-secrets "$BUS" --no-default-features
check "porter-oauth --no-default-features" porter-oauth "$BUS" --no-default-features
check "porter-families --no-default-features" porter-families "$BUS" --no-default-features
check "porter-router" porter-router "$BUS"
check "porter-turns" porter-turns "$BUS"

exit "$fail"
