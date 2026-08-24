#!/usr/bin/env bash
set -uo pipefail

BRIDGE=br-xdplb
PIDFILE=/tmp/xdp-lb-backends.pids

if [[ $EUID -ne 0 ]]; then
	echo "run as root" >&2
	exit 1
fi

pkill -f "[b]ackend.py" 2>/dev/null
pkill -f "[f]ake-prometheus.py" 2>/dev/null
rm -f "$PIDFILE"

for ns in client be1 be2 lb; do
	ip netns del "$ns" 2>/dev/null
done

ip link del "$BRIDGE" 2>/dev/null

echo "topology down"
