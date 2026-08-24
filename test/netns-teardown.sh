#!/usr/bin/env bash
set -uo pipefail

BRIDGE=br-xdplb
PIDFILE=/tmp/xdp-lb-backends.pids

if [[ $EUID -ne 0 ]]; then
	echo "run as root" >&2
	exit 1
fi

if [[ -f $PIDFILE ]]; then
	while read -r pid; do
		[[ -n $pid ]] && kill -- "-$pid" 2>/dev/null
		[[ -n $pid ]] && kill "$pid" 2>/dev/null
	done <"$PIDFILE"
	rm -f "$PIDFILE"
fi
pkill -f 'test/backend.py' 2>/dev/null

for ns in client be1 be2 lb; do
	ip netns del "$ns" 2>/dev/null
done

ip link del "$BRIDGE" 2>/dev/null

echo "topology down"
