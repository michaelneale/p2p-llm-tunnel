# P2P HTTP Tunnel - Development Commands
# Run `just --list` to see all available commands

# Default: show available commands
default:
    @just --list

# Build everything (tunnel + signal server)
build: build-tunnel build-signal

# Build the tunnel CLI (release mode)
build-tunnel:
    cd tunnel && cargo build --release

# Build the signal server
build-signal:
    cd signal-server && npm install && npm run build

# Run integration test using public signal server
test:
    ./scripts/test-tunnel.sh

# Run integration test using local signal server
test-local: build
    ./scripts/test-local.sh

# Run tunnel unit tests
test-unit:
    cd tunnel && cargo test

# Clean build artifacts
clean:
    cd tunnel && cargo clean
    rm -rf signal-server/node_modules signal-server/dist

# Start serve mode (requires ROOM and UPSTREAM)
# Example: just serve room=myroom upstream=http://localhost:11434
serve room upstream:
    ./tunnel/target/release/tunnel serve --room {{room}} --upstream {{upstream}}

# Start proxy mode (requires ROOM)
# Example: just proxy room=myroom
# Example: just proxy room=myroom listen=127.0.0.1:9000
proxy room listen="127.0.0.1:8000":
    ./tunnel/target/release/tunnel proxy --room {{room}} --listen {{listen}}

# Start local signal server (for development)
signal port="8787":
    cd signal-server && node dist/index.js --port {{port}}

# Deploy signal server to Fly.io
deploy-signal:
    cd signal-server && fly deploy

# Check signal server status on Fly.io
signal-status:
    cd signal-server && fly status

# View signal server logs on Fly.io
signal-logs:
    cd signal-server && fly logs
