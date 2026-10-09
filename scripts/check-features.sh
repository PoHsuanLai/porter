#!/usr/bin/env bash
# The feature sets the consumers of porter's crates build, each one clippy with -D warnings.
#
# The workspace gate builds `--workspace --all-features`, which unifies every feature into one
# build and so never sees the build a consumer makes on its own. docket builds inferd that way
# (`cargo install --git porter`, -D warnings): porter-client with `dbus` and `infer` and no
# `socket` left two framed.rs methods unused, and the gate passed (porter 5bdda62; fixed in
# b12886e). Each set below is checked here, alone, so the same break fails this script.
#
# The sets come from the consumers' manifests at origin/master (porter-client and the other
# porter crates each consumer takes with default-features = false). The comment above each set
# names the consumer it comes from. Keep the list as data: add a set when a consumer adds one.
#
# Output: one line per build, `ok <set>` or `FAIL <set>`, with the first compiler errors of a
# failed build and the path of its log (the logs go to $CARGO_TARGET_DIR/check-features/, so
# nothing is written into the tree). The exit status is the verdict.
set -uo pipefail
cd "$(dirname "$0")/.."

# Each entry: "<set> :: <cargo arguments>". Every build is `cargo clippy <arguments>
# --all-targets -- -D warnings`.
BUILDS=(
  # The three daemons, each alone with its default features: `-p` alone, so no other package's
  # features are unified in. inferd is docket's sibling binary; accountd and syncd are the
  # desktop's.
  "inferd (default) :: -p inferd"
  "accountd (default) :: -p accountd"
  "syncd (default) :: -p syncd"

  # porter-client, in the sets its consumers name. Consumers that say default-features = false:
  # mailo's pure client (no features), and detent's Settings (dbus). inferd and syncd take dbus
  # with the defaults on (docket, almanac, cua and their daemons too). The dbus-plus-infer set
  # without socket is the one docket's inferd build broke (see the header).
  "porter-client (no features: mailo) :: -p porter-client --no-default-features"
  "porter-client (dbus: detent) :: -p porter-client --no-default-features --features dbus"
  "porter-client (dbus,infer: inferd's build as docket makes it) :: -p porter-client --no-default-features --features dbus,infer"
  "porter-client (dbus with defaults: inferd, syncd, docket, almanac, cua) :: -p porter-client --features dbus"
  "porter-client (socket: the other desktops and macOS) :: -p porter-client --no-default-features --features socket"
  "porter-client (socket,infer) :: -p porter-client --no-default-features --features socket,infer"
  "porter-client (defaults: docket, almanac, cua) :: -p porter-client"
  "porter-client (engines) :: -p porter-client --features engines"

  # The other porter crates a consumer takes with default-features = false (mailo's workspace
  # block, which mail-core, mail-app and mail-runtime name), in the features each one turns on.
  "porter-secrets (testing: mailo mail-core, mail-app) :: -p porter-secrets --no-default-features --features testing"
  "porter-secrets (oo7: mailo mail-runtime, Linux) :: -p porter-secrets --no-default-features --features oo7"
  "porter-secrets (keyring: mailo mail-runtime, macOS and Windows) :: -p porter-secrets --no-default-features --features keyring"
  "porter-discover (no features: mailo mail-core, mail-runtime) :: -p porter-discover --no-default-features"
  "porter-http (no features: mailo mail-core, mail-runtime) :: -p porter-http --no-default-features"
  "porter-oauth (no features: mailo) :: -p porter-oauth --no-default-features"
  "porter-oauth (io: mailo mail-runtime) :: -p porter-oauth --no-default-features --features io"
  "porter-proxy (io: mailo mail-runtime, its link tests) :: -p porter-proxy --no-default-features --features io"
)

logs="${CARGO_TARGET_DIR:-target}/check-features"
mkdir -p "$logs"
fail=0
n=0
for build in "${BUILDS[@]}"; do
  n=$((n + 1))
  set_name=${build%% :: *}
  read -r -a args <<<"${build#* :: }"
  log="$logs/$n.log"
  if cargo clippy "${args[@]}" --all-targets -- -D warnings >"$log" 2>&1; then
    echo "ok $set_name"
  else
    echo "FAIL $set_name"
    echo "  log: $log"
    grep -m 5 -E '^(error|warning)' "$log" | sed 's/^/  /'
    fail=1
  fi
done
exit "$fail"
