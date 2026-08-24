#!/usr/bin/env bash
set -euo pipefail

VIP=${VIP:-10.0.0.100}
REQUESTS=${REQUESTS:-40}

if [[ $EUID -ne 0 ]]; then
	echo "run as root" >&2
	exit 1
fi

declare -A hits=()
failures=0

for _ in $(seq "$REQUESTS"); do
	if answer=$(ip netns exec client curl -s --max-time 2 "http://$VIP/"); then
		answer=${answer//[$'\r\n']/}
		hits[$answer]=$((${hits[$answer]:-0} + 1))
	else
		failures=$((failures + 1))
	fi
done

echo "requests: $REQUESTS"
for backend in "${!hits[@]}"; do
	echo "  $backend: ${hits[$backend]}"
done
echo "  failed: $failures"

if [[ $failures -eq $REQUESTS ]]; then
	echo
	echo "everything failed. check, in order:"
	echo "  ip netns exec lb bpftool net show"
	echo "  curl -s localhost:9500/metrics | grep xdplb_"
	echo "  ip netns exec lb cat /sys/kernel/debug/tracing/trace_pipe"
	exit 1
fi
