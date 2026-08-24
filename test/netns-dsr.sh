#!/usr/bin/env bash
set -euo pipefail

VIP=10.0.0.100
LB=10.0.0.1
CLIENT_SUBNET=10.1.0.0/24
SERVICE_PORT=80
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $EUID -ne 0 ]]; then
	echo "run as root" >&2
	exit 1
fi

if ! ip netns list | grep -q '^be1'; then
	echo "run netns-setup.sh first" >&2
	exit 1
fi

pkill -f "[b]ackend.py" 2>/dev/null || true

declare -A ADDRESS=([be1]=10.0.0.11 [be2]=10.0.0.12)

for ns in be1 be2; do
	ip -n "$ns" tunnel del ipip0 2>/dev/null || true
	ip -n "$ns" tunnel add ipip0 mode ipip local "${ADDRESS[$ns]}" remote "$LB"
	ip -n "$ns" link set ipip0 up
	ip -n "$ns" addr replace "$VIP/32" dev lo
	ip -n "$ns" route replace "$CLIENT_SUBNET" dev eth0
	ip netns exec "$ns" sysctl -qw net.ipv4.conf.all.rp_filter=0
	ip netns exec "$ns" sysctl -qw net.ipv4.conf.ipip0.rp_filter=0

	setsid ip netns exec "$ns" python3 "$HERE/backend.py" "$ns" "$SERVICE_PORT" \
		</dev/null >"/tmp/xdp-lb-$ns.log" 2>&1 &
done

sleep 1

echo "topology converted to direct server return"
echo "  backends decapsulate IPIP on ipip0 and hold $VIP on lo"
echo "  backends reach $CLIENT_SUBNET directly, so replies never touch the load balancer"
echo "  backends now listen on :$SERVICE_PORT because dsr does not rewrite ports"
echo
echo "next: sudo ip netns exec lb ./target/debug/xdp-lb --config test/config.dsr.yaml"
echo "      make smoke"
