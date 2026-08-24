#!/usr/bin/env bash
set -euo pipefail

NS=${1:?usage: backend-down.sh <be1|be2>}

if [[ $EUID -ne 0 ]]; then
	echo "run as root" >&2
	exit 1
fi

pkill -f "[b]ackend.py $NS" || {
	echo "no backend running in $NS" >&2
	exit 1
}

echo "$NS stopped"
