#!/usr/bin/env bash
set -euo pipefail

NS=${1:?usage: backend-up.sh <be1|be2> [port]}
PORT=${2:-8080}
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $EUID -ne 0 ]]; then
	echo "run as root" >&2
	exit 1
fi

if pgrep -f "[b]ackend.py $NS" >/dev/null; then
	echo "$NS already running" >&2
	exit 1
fi

setsid ip netns exec "$NS" python3 "$HERE/backend.py" "$NS" "$PORT" \
	</dev/null >"/tmp/xdp-lb-$NS.log" 2>&1 &

sleep 1
echo "$NS started on :$PORT"
