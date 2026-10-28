#!/usr/bin/env bash
# The portable core (quire design/36 section 1, rule 3 and section 4): everything except the
# desktop itself builds without the desktop. Each core below must `cargo check` with the
# features named, and its normal dependency tree must reach no D-Bus (zbus, zvariant). The
# porter-client build an accounts-only consumer uses (no features) must also reach no inference:
# no porter-infer, and so no stoker.
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

check "porter-client" porter-client "$BUS|$INFER"
check "porter-client-inproc" porter-client-inproc "$BUS|$INFER"
check "porter-ids" porter-ids "$BUS"
check "porter-core" porter-core "$BUS"
check "porter-provider" porter-provider "$BUS"
check "porter-secrets --no-default-features" porter-secrets "$BUS" --no-default-features
check "porter-oauth --no-default-features" porter-oauth "$BUS" --no-default-features
check "porter-families --no-default-features" porter-families "$BUS" --no-default-features
check "porter-router" porter-router "$BUS"
check "porter-turns" porter-turns "$BUS"

# The client API is the one an app on macOS or Windows links, so it must compile there, not only
# on a host that happens to have unix descriptors: the Linux-only code (a memfd, a bus) is in
# porter-client-dbus. Every target of these that rustup has installed is checked; a target that is
# not installed is said, and skipped (this script installs nothing).
CROSS='x86_64-pc-windows-msvc x86_64-pc-windows-gnu aarch64-apple-darwin x86_64-apple-darwin'
installed=$(rustup target list --installed 2>/dev/null)
for target in $CROSS; do
  if ! grep -qx "$target" <<<"$installed"; then
    echo "skip cross: $target is not installed"
    continue
  fi
  if cargo check --target "$target" -p porter-client -p porter-sheet -p porter-wire >/dev/null 2>&1; then
    echo "check ok: porter-client porter-sheet porter-wire on $target"
  else
    echo "FAIL check: porter-client porter-sheet porter-wire do not build on $target"
    fail=1
  fi
done

exit "$fail"
