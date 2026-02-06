#!/bin/bash
# Local end-to-end test for the P2P HTTP tunnel
# This script starts all components and tests the tunnel locally.
#
# Prerequisites:
#   - Node.js installed
#   - Rust/cargo installed
#   - signal-server dependencies installed (cd signal-server && npm install)
#   - tunnel built (cd tunnel && cargo build)

set -e

COLOR_GREEN='\033[0;32m'
COLOR_RED='\033[0;31m'
COLOR_YELLOW='\033[0;33m'
COLOR_RESET='\033[0m'

cleanup() {
    echo -e "\n${COLOR_YELLOW}Cleaning up...${COLOR_RESET}"
    [ -n "$UPSTREAM_PID" ] && kill $UPSTREAM_PID 2>/dev/null || true
    [ -n "$SIGNAL_PID" ] && kill $SIGNAL_PID 2>/dev/null || true
    [ -n "$SERVE_PID" ] && kill $SERVE_PID 2>/dev/null || true
    [ -n "$PROXY_PID" ] && kill $PROXY_PID 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT

echo -e "${COLOR_GREEN}=== P2P HTTP Tunnel Local Test ===${COLOR_RESET}"

# 1. Start a simple upstream HTTP server (simulates an API)
echo -e "\n${COLOR_GREEN}[1/5] Starting upstream HTTP server on :3001...${COLOR_RESET}"
python3 -c '
import http.server
import json
import socketserver

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/v1/models":
            response = {"object": "list", "data": [{"id": "test-model", "object": "model"}]}
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(response).encode())
        elif self.path == "/health":
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"ok")
        else:
            self.send_response(404)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"not found")
    def log_message(self, format, *args):
        pass  # Suppress logs

with socketserver.TCPServer(("", 3001), Handler) as httpd:
    httpd.serve_forever()
' &
UPSTREAM_PID=$!
sleep 1

# Verify upstream is running
if curl -s http://127.0.0.1:3001/health > /dev/null 2>&1; then
    echo -e "  ${COLOR_GREEN}✓ Upstream server running${COLOR_RESET}"
else
    echo -e "  ${COLOR_RED}✗ Failed to start upstream server${COLOR_RESET}"
    exit 1
fi

# 2. Start signaling server
echo -e "\n${COLOR_GREEN}[2/5] Starting signaling server on :8787...${COLOR_RESET}"
(cd signal-server && node dist/index.js --port 8787) &
SIGNAL_PID=$!
sleep 1
echo -e "  ${COLOR_GREEN}✓ Signaling server running${COLOR_RESET}"

# 3. Start tunnel serve (provider)
echo -e "\n${COLOR_GREEN}[3/5] Starting tunnel serve...${COLOR_RESET}"
RUST_LOG=info ./tunnel/target/debug/tunnel serve \
    --signal ws://127.0.0.1:8787 \
    --room test-room \
    --upstream http://127.0.0.1:3001 &
SERVE_PID=$!
sleep 2
echo -e "  ${COLOR_GREEN}✓ Tunnel serve started${COLOR_RESET}"

# 4. Start tunnel proxy (consumer)
echo -e "\n${COLOR_GREEN}[4/5] Starting tunnel proxy on :8000...${COLOR_RESET}"
RUST_LOG=info ./tunnel/target/debug/tunnel proxy \
    --signal ws://127.0.0.1:8787 \
    --room test-room \
    --listen 127.0.0.1:8000 &
PROXY_PID=$!

# Wait for WebRTC connection + handshake
echo -e "  Waiting for tunnel to establish..."
sleep 8
echo -e "  ${COLOR_GREEN}✓ Tunnel proxy started${COLOR_RESET}"

# 5. Test the tunnel
echo -e "\n${COLOR_GREEN}[5/5] Testing tunnel...${COLOR_RESET}"

echo -e "\n  Testing: curl http://127.0.0.1:8000/v1/models"
RESPONSE=$(curl -s http://127.0.0.1:8000/v1/models 2>&1)
echo -e "  Response: $RESPONSE"

if echo "$RESPONSE" | grep -q 'test-model'; then
    echo -e "\n${COLOR_GREEN}✓ SUCCESS! Request tunneled through WebRTC and reached upstream.${COLOR_RESET}"
else
    echo -e "\n${COLOR_RED}✗ FAILED: Expected response containing 'test-model'${COLOR_RESET}"
    echo -e "  Got: $RESPONSE"
    exit 1
fi

echo -e "\n  Testing: curl http://127.0.0.1:8000/health"
RESPONSE2=$(curl -s http://127.0.0.1:8000/health 2>&1)
echo -e "  Response: $RESPONSE2"

if [ "$RESPONSE2" = "ok" ]; then
    echo -e "\n${COLOR_GREEN}✓ SUCCESS! Health check passed.${COLOR_RESET}"
else
    echo -e "\n${COLOR_RED}✗ FAILED: Expected 'ok'${COLOR_RESET}"
    exit 1
fi

echo -e "\n${COLOR_GREEN}=== All tests passed! ===${COLOR_RESET}"
