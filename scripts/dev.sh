#!/bin/bash
# Development launcher: generates certs, starts relay + vite, opens Chrome.
# Usage: ./scripts/dev.sh

set -e
cd "$(dirname "$0")/.."

CERT_DIR=certs
CERT="$CERT_DIR/localhost+2.pem"
KEY="$CERT_DIR/localhost+2-key.pem"

# === Generate certs if missing or expired ===
if [ ! -f "$CERT" ] || [ ! -f "$KEY" ]; then
  echo "Generating certificates..."
  mkdir -p "$CERT_DIR"
  (cd "$CERT_DIR" && mkcert localhost 127.0.0.1 ::1)
fi

# === Kill existing processes ===
lsof -i :4433 -t 2>/dev/null | xargs kill 2>/dev/null || true
pkill -f "vite.*5173" 2>/dev/null || true
sleep 1

# === Start relay ===
echo "Starting relay..."
cargo run --bin moqt-relay -- --cert "$CERT" --key "$KEY" &
RELAY_PID=$!
sleep 2

# === Start Vite ===
echo "Starting Vite..."
(cd web && npx vite --port 5173) &
VITE_PID=$!
sleep 2

echo ""
echo "=== Ready ==="
echo "Relay PID: $RELAY_PID (port 4433)"
echo "Vite PID:  $VITE_PID (port 5173)"
echo ""
echo "Open: http://localhost:5173/"
echo ""
echo "Press Ctrl+C to stop all"

# Open Chrome
open "http://localhost:5173/"

# Wait and cleanup on Ctrl+C
trap "kill $RELAY_PID $VITE_PID 2>/dev/null; exit 0" INT TERM
wait
