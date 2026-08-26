#!/usr/bin/env bash
# capture-filter-log.sh — records defmt-serial output to a file for later
# plotting (see tools/plot_filters.py). Ctrl+C to stop capturing; the file
# is flushed as it goes, so a hard exit still leaves you a usable log.
#
# Usage:
#   ./capture-filter-log.sh [elf_path] [output_file]
#
# Defaults:
#   elf_path    -> target/thumbv6m-none-eabi/debug/pico-pot-meter
#   output_file -> filter_log_<timestamp>.txt

set -euo pipefail

ELF="${1:-target/thumbv6m-none-eabi/debug/pico-pot-meter}"
OUT="${2:-filter_log_$(date +%Y%m%d_%H%M%S).txt}"

if [[ ! -f "$ELF" ]]; then
    echo "error: ELF not found at $ELF (build first: cargo build --release)" >&2
    exit 1
fi

# Auto-detect the board's serial port. Adjust the glob if yours enumerates
# differently (e.g. /dev/tty.usbmodem* on macOS).
PORT=""
for candidate in /dev/ttyACM0 /dev/ttyACM1 /dev/tty.usbmodem*; do
    if [[ -e "$candidate" ]]; then
        PORT="$candidate"
        break
    fi
done

if [[ -z "$PORT" ]]; then
    echo "error: no serial port found. Is the board plugged in and running (not in BOOTSEL mode)?" >&2
    exit 1
fi

echo "Capturing from $PORT -> $OUT (Ctrl+C to stop)"
echo "---"

# `tee` writes to the file AND stdout so you can watch it live while it saves.
# `stdbuf -oL` on defmt-print keeps output line-buffered so `tee`/the file
# don't lag behind what you're seeing on screen.
defmt-print -e "$ELF" < "$PORT" | stdbuf -oL cat | tee "$OUT"
