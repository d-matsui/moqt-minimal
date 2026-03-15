#!/bin/bash
# E2E video test: ffmpeg testsrc (VP8/IVF) → moqt-pub → relay → moqt-sub → ffplay
set -e

cd "$(dirname "$0")/.."

echo "=== Building... ==="
cargo build 2>&1 | tail -1

# Clean up from previous runs
pkill -f "target/debug/moqt-" 2>/dev/null || true
sleep 0.5

echo "=== Starting Relay ==="
./target/debug/moqt-relay 2>&1 &
RELAY_PID=$!
sleep 1

if ! kill -0 $RELAY_PID 2>/dev/null; then
    echo "ERROR: Relay failed to start"
    exit 1
fi

echo "=== Starting Publisher (ffmpeg VP8 testsrc 10s) ==="
ffmpeg -re -f lavfi -i testsrc=duration=10:size=320x240:rate=30 \
    -c:v libvpx -g 30 -f ivf pipe:1 2>/dev/null \
    | ./target/debug/moqt-pub --pipe 127.0.0.1:4433 2>/dev/stderr &
PUB_PID=$!
sleep 2

echo "=== Starting Subscriber (piping to ffplay) ==="
./target/debug/moqt-sub --pipe 127.0.0.1:4433 2>/dev/stderr \
    | ffplay -f ivf -autoexit - 2>/dev/null

echo "=== Done ==="
kill $RELAY_PID $PUB_PID 2>/dev/null || true
wait 2>/dev/null
