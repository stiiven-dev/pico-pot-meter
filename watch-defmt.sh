#!/usr/bin/env bash
# Watches for the Pico's CDC-ACM port under either /dev/ttyACM0 or
# /dev/ttyACM1 (USB re-enumeration on reset can flip which node it lands
# on) and pipes whichever one shows up into socat + defmt-print. Restarts
# the scan every time the device disappears (reset, panic-persist reboot,
# unplug, etc.).
#
# Usage:
#   ./watch-defmt.sh [path-to-elf]
#
# Defaults to the debug build if no ELF path is given.

set -u

ELF="${1:-target/thumbv6m-none-eabi/debug/pico-pot-meter}"
CANDIDATES=(/dev/ttyACM0 /dev/ttyACM1)
BAUD=115200

if [[ ! -f "$ELF" ]]; then
    echo "warning: ELF not found at '$ELF' — pass the correct path as an argument" >&2
fi

find_port() {
    for dev in "${CANDIDATES[@]}"; do
        if [[ -e "$dev" ]]; then
            echo "$dev"
            return 0
        fi
    done
    return 1
}

echo "watching for ${CANDIDATES[*]} (ctrl-c to quit)"
while true; do
    port=$(find_port) || { sleep 0.2; continue; }
    echo "[connected]    $port"
    socat "${port},rawer,b${BAUD}" STDOUT | defmt-print -e "$ELF"
    echo "[disconnected] $port — rescanning..."
    sleep 0.3
done
