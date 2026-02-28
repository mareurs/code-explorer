#!/usr/bin/env bash
set -euo pipefail

PORT=8099
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUTPUT="$PROJECT_ROOT/docs/images/dashboard.png"
SCRIPTS_DIR="$PROJECT_ROOT/scripts"

# --- Check binary ---
BINARY=$(command -v code-explorer 2>/dev/null || echo "$PROJECT_ROOT/target/release/code-explorer")
if [[ ! -x "$BINARY" ]]; then
  echo "ERROR: code-explorer binary not found."
  echo "Build it first: cargo build --release"
  exit 1
fi

# --- Check Node deps ---
if [[ ! -d "$SCRIPTS_DIR/node_modules" ]]; then
  echo "ERROR: Playwright not installed."
  echo "Run: cd scripts && npm install && npx playwright install chromium"
  exit 1
fi

# --- Start dashboard server in background ---
echo "Starting dashboard on port $PORT ..."
"$BINARY" dashboard --project "$PROJECT_ROOT" --port "$PORT" --no-open &
SERVER_PID=$!

# Always kill server on exit
trap "kill $SERVER_PID 2>/dev/null || true" EXIT

# --- Wait for server to be ready ---
echo "Waiting for server..."
for i in $(seq 1 20); do
  if curl -sf "http://localhost:$PORT/api/health" > /dev/null 2>&1; then
    echo "Server ready."
    break
  fi
  if [[ $i -eq 20 ]]; then
    echo "ERROR: Server did not start within 10s."
    exit 1
  fi
  sleep 0.5
done

# --- Capture screenshot ---
echo "Capturing screenshot..."
node "$SCRIPTS_DIR/capture-dashboard.js" "$PORT" "$OUTPUT"

echo ""
echo "Screenshot saved to: $OUTPUT"
echo "Commit it with:"
echo "  git add docs/images/dashboard.png"
echo "  git commit -m \"docs: update dashboard screenshot\""
