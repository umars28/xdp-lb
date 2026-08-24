#!/usr/bin/env bash
set -euo pipefail

BRIDGE=br-xdplb
VIP=10.0.0.100
BACKEND_PORT=8080
PIDFILE=/tmp/xdp-lb-backends.pids
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $EUID -ne 0 ]]; then
	echo "run as root" >&2
	exit 1
fi

if ip link show "$BRIDGE" &>/dev/null; then
	echo "$BRIDGE already exists; run netns-teardown.sh first" >&2
	exit 1
fi

ip link add "$BRIDGE" type bridge
ip link set "$BRIDGE" up

attach() {
	local ns=$1 addr=$2 prefix=$3
	ip netns add "$ns"
	ip link add "veth-$ns" type veth peer name eth0 netns "$ns"
	ip link set "veth-$ns" master "$BRIDGE" up
	ip -n "$ns" link set lo up
	ip -n "$ns" link set eth0 up
	ip -n "$ns" addr add "$addr/$prefix" dev eth0
}

attach lb 10.0.0.1 24
attach be1 10.0.0.11 24
attach be2 10.0.0.12 24
attach client 10.1.0.10 24

LB_MAC=$(ip -n lb link show eth0 | awk '/link\/ether/ {print $2}')

ip -n client route add 10.0.0.1 dev eth0
ip -n client route add default via 10.0.0.1
ip -n client neigh replace 10.0.0.1 lladdr "$LB_MAC" dev eth0
ip -n client neigh replace "$VIP" lladdr "$LB_MAC" dev eth0

ip -n lb route add 10.1.0.0/24 dev eth0

for ns in be1 be2; do
	ip -n "$ns" route replace default via 10.0.0.1
done

for addr in 10.0.0.11 10.0.0.12; do
	ip netns exec lb ping -c 1 -W 1 "$addr" >/dev/null || true
done

: >"$PIDFILE"
for ns in be1 be2; do
	setsid ip netns exec "$ns" python3 "$HERE/backend.py" "$ns" "$BACKEND_PORT" \
		</dev/null >"/tmp/xdp-lb-$ns.log" 2>&1 &
	echo $! >>"$PIDFILE"
done

sleep 1

echo "topology up"
echo "  client  10.1.0.10/24  -> default via 10.0.0.1"
echo "  lb      10.0.0.1/24   -> attach xdp here (eth0)"
echo "  be1     10.0.0.11:$BACKEND_PORT"
echo "  be2     10.0.0.12:$BACKEND_PORT"
echo "  vip     $VIP:80"
echo
echo "next: make run    (in another shell)"
echo "      make smoke"
