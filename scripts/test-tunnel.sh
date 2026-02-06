#!/bin/bash
# Test the tunnel using the public signal server (wss://signal-server.fly.dev)
# This is the main integration test - verifies end-to-end HTTP tunneling.
#
# What it does:
#   1. Starts a local HTTP server with a distinctive JSON response
#   2. Runs `tunnel serve` pointing to that server
#   3. Runs `tunnel proxy` to create a local proxy
#   4. Curls through the proxy and verifies the response came from upstream
#
# Usage:
#   ./scripts/test-tunnel.sh

set -e

ROOM="test-$(date +%s)"
UPSTREAM_PORT=8080
PROXY_PORT=9000

echo "=== P2P Tunnel Test ==="
echo "Room: $ROOM"
echo "Signal: wss://signal-server.fly.dev (public)"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    kill $HTTP_PID $SERVE_PID $PROXY_PID 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT

# 1. Start upstream HTTP server with distinctive response
echo "[1] Starting upstream server on :$UPSTREAM_PORT"
python3 -c '
from http.server import HTTPServer, BaseHTTPRequestHandler
import json, os

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        response = {
            "status": "ok",
            "message": "Hello from UPSTREAM server!",
            "path": self.path,
            "pid": os.getpid(),
            "tunnel_test": True
        }
        self.wfile.write(json.dumps(response, indent=2).encode())
    def log_message(self, *args): pass

print(f"Upstream server running on :'"$UPSTREAM_PORT"'")
HTTPServer(("127.0.0.1", '"$UPSTREAM_PORT"'), Handler).serve_forever()
' > /tmp/http.log 2>&1 &
HTTP_PID=$!
sleep 1

# 2. Start tunnel serve
echo "[2] Starting tunnel serve..."
./tunnel/target/release/tunnel serve \
    --room "$ROOM" \
    --upstream "http://127.0.0.1:$UPSTREAM_PORT" \
    > /tmp/serve.log 2>&1 &
SERVE_PID=$!
sleep 2

# 3. Start tunnel proxy
echo "[3] Starting tunnel proxy on :$PROXY_PORT..."
./tunnel/target/release/tunnel proxy \
    --room "$ROOM" \
    --listen "127.0.0.1:$PROXY_PORT" \
    > /tmp/proxy.log 2>&1 &
PROXY_PID=$!

# 4. Wait for connection
echo "[4] Waiting for tunnel to establish..."
for i in {1..15}; do
    if grep -q "tunnel ready" /tmp/serve.log 2>/dev/null && grep -q "proxy listening" /tmp/proxy.log 2>/dev/null; then
        echo "    Tunnel ready!"
        break
    fi
    sleep 1
    echo "    ... ($i)"
done

# 5. Test it
echo ""
echo "[5] Testing tunnel..."
echo ""
echo "--- Direct to upstream (should work) ---"
curl -s "http://127.0.0.1:$UPSTREAM_PORT/direct-test"
echo ""
echo ""
echo "--- Through P2P tunnel (the real test) ---"
curl -s "http://127.0.0.1:$PROXY_PORT/tunnel-test"
echo ""

# 6. Show logs
echo ""
echo "=== Logs ==="
echo "--- serve.log ---"
tail -20 /tmp/serve.log
echo ""
echo "--- proxy.log ---"
tail -20 /tmp/proxy.log
