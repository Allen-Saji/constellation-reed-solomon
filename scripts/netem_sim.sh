#!/usr/bin/env bash
#
# Simulate attester packet loss with tc netem and prove Constellation recovery.
#
# Sets up an ISOLATED network namespace + veth pair (so it never touches your
# real loopback or default routes), applies `tc netem loss <pct>` on the link
# toward the attester, then runs the proposer (256 pshreds) against the attester
# and reports whether the pslice was reconstructed from the survivors.
#
# At 64 data of 256 total, recovery holds up to ~75% loss (any 64 must arrive).
#
# Usage:  sudo ./scripts/netem_sim.sh [loss_pct] [pslice_len]
#   e.g.  sudo ./scripts/netem_sim.sh 70 16384
#
# Requires: root (sudo), iproute2 (ip, tc). Idempotent: cleans up on exit.

set -euo pipefail

LOSS_PCT="${1:-70}"
PSLICE_LEN="${2:-16384}"
NS="cnstl_ns"
HOST_IF="veth-h"
NS_IF="veth-n"
HOST_IP="10.55.0.1"
NS_IP="10.55.0.2"
PORT="9000"

if [[ "${EUID}" -ne 0 ]]; then
  echo "error: needs root for ip netns / tc. Re-run with sudo." >&2
  exit 1
fi
for cmd in ip tc; do
  command -v "$cmd" >/dev/null || { echo "error: '$cmd' not found (install iproute2)." >&2; exit 1; }
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}/.."

cleanup() {
  tc qdisc del dev "${HOST_IF}" root 2>/dev/null || true
  ip netns del "${NS}" 2>/dev/null || true
  ip link del "${HOST_IF}" 2>/dev/null || true
}
trap cleanup EXIT
cleanup  # clear any leftovers from a previous run

echo "==> building proposer + attester (release)"
# Build as the invoking user if run under sudo, so artifacts stay user-owned.
if [[ -n "${SUDO_USER:-}" ]]; then
  sudo -u "${SUDO_USER}" cargo build --release --bins
else
  cargo build --release --bins
fi

echo "==> setting up isolated netns '${NS}' with ${LOSS_PCT}% loss toward attester"
ip netns add "${NS}"
ip link add "${HOST_IF}" type veth peer name "${NS_IF}"
ip link set "${NS_IF}" netns "${NS}"
ip addr add "${HOST_IP}/24" dev "${HOST_IF}"
ip netns exec "${NS}" ip addr add "${NS_IP}/24" dev "${NS_IF}"
ip link set "${HOST_IF}" up
ip netns exec "${NS}" ip link set "${NS_IF}" up
ip netns exec "${NS}" ip link set lo up
# Drop a fraction of egress datagrams on the proposer -> attester direction.
tc qdisc add dev "${HOST_IF}" root netem loss "${LOSS_PCT}%"

echo "==> running attester (in ns) and proposer (on host)"
ip netns exec "${NS}" ./target/release/attester "${NS_IP}:${PORT}" "${PSLICE_LEN}" &
ATTESTER_PID=$!
sleep 0.4
./target/release/proposer "${NS_IP}:${PORT}" "${PSLICE_LEN}"

# Surface the attester's exit code (0 = reconstructed, non-zero = failed).
set +e
wait "${ATTESTER_PID}"
RC=$?
set -e

echo "==> attester exit code: ${RC} (0 = pslice reconstructed)"
exit "${RC}"
