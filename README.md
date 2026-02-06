# P2P HTTP Tunnel

Expose any HTTP service (like a local LLM) through a peer-to-peer WebRTC tunnel. No port forwarding, no static IP needed.

## Install

```bash
# macOS (Apple Silicon)
curl -L https://github.com/michaelneale/p2p-llm-tunnel/releases/latest/download/tunnel-darwin-arm64.gz | gunzip > tunnel
chmod +x tunnel
```

Or build from source (requires Rust 1.70+):
```bash
cd tunnel && cargo build --release
```

## Quick Start

**On the machine with your API** (e.g., home server running Ollama):
```bash
tunnel serve --room my-secret-room --upstream http://127.0.0.1:11434
```

**On your laptop/remote machine:**
```bash
tunnel proxy --room my-secret-room --listen 127.0.0.1:8000
```

**Use it:**
```bash
curl http://127.0.0.1:8000/api/tags
```

That's it. Both sides connect to a public signal server, find each other by room name, establish a direct WebRTC connection, and your HTTP traffic flows through.

## Usage

### `tunnel serve`

Exposes an upstream HTTP service through the tunnel.

```bash
tunnel serve --room ROOM --upstream URL [--signal URL] [--advertise PATH]
```

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--room` | `TUNNEL_ROOM` | required | Room name (both sides must match) |
| `--upstream` | `TUNNEL_UPSTREAM` | required | Local HTTP service to expose |
| `--signal` | `TUNNEL_SIGNAL` | `wss://signal-server.fly.dev` | Signal server URL |
| `--advertise` | — | `/` | Path prefix to strip |

### `tunnel proxy`

Creates a local HTTP server that tunnels requests to the remote serve.

```bash
tunnel proxy --room ROOM [--listen ADDR] [--signal URL]
```

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--room` | `TUNNEL_ROOM` | required | Room name (both sides must match) |
| `--listen` | `TUNNEL_LISTEN` | `127.0.0.1:8000` | Local address to listen on |
| `--signal` | `TUNNEL_SIGNAL` | `wss://signal-server.fly.dev` | Signal server URL |

### Environment Variables

All flags can be set via environment variables:
```bash
export TUNNEL_ROOM=my-room
export TUNNEL_UPSTREAM=http://localhost:11434
tunnel serve
```

### Logging

```bash
RUST_LOG=debug tunnel serve ...
RUST_LOG=info tunnel proxy ...
```

## Security

**Current model:** The room name is the only authentication. Anyone who knows the room name can connect.

**Recommendations:**
- Use long, random room names (e.g., `llm-a8f2b9c1d4e5`)
- The WebRTC data channel is encrypted (DTLS) — traffic can't be intercepted
- The signal server only sees WebRTC handshake metadata, never your HTTP traffic

**Future options under consideration:**
- `--secret` flag for pre-shared key authentication
- Certificate pinning via WebRTC DTLS fingerprints

## How It Works

```
┌─────────────┐     WebRTC      ┌─────────────┐
│   proxy     │◄───encrypted───►│    serve    │
│  :8000      │   data channel  │             │
└──────┬──────┘                 └──────┬──────┘
       │                               │
       │ HTTP                          │ HTTP
       ▼                               ▼
   Your Apps                     Upstream API
                                (Ollama, etc)
```

1. Both peers connect to the signal server and join the same room
2. They exchange WebRTC connection info (SDP offers, ICE candidates)
3. Direct peer-to-peer connection established (NAT traversal via STUN)
4. Signal server no longer needed — all traffic flows directly
5. HTTP requests are multiplexed over the encrypted data channel

## Networking Notes

- **Works on most networks** — STUN handles typical NAT configurations
- **Symmetric NAT** — May require a TURN relay server (not yet implemented)
- **Keepalive** — Pings every 10s to maintain the connection during idle periods

## Self-Hosting the Signal Server

The public signal server at `wss://signal-server.fly.dev` is free to use. To run your own:

```bash
cd signal-server
npm install && npm run build
node dist/index.js --port 8787
```

Then use `--signal ws://your-server:8787` on both sides.

---

## Development

### Commands (using [just](https://github.com/casey/just))

```bash
just build          # Build tunnel + signal server
just test           # Integration test (public signal server)
just test-local     # Integration test (local signal server)
just test-unit      # Run unit tests

# Run tunnel directly
just serve room=myroom upstream=http://localhost:11434
just proxy room=myroom
just proxy room=myroom listen=127.0.0.1:9000

# Signal server management
just signal         # Run local signal server
just deploy-signal  # Deploy to Fly.io
just signal-logs    # View Fly.io logs
```

### Project Structure

```
├── tunnel/                 # Rust CLI (serve + proxy)
│   └── src/
│       ├── main.rs         # Entry point
│       ├── serve.rs        # Provider mode
│       ├── proxy.rs        # Consumer mode
│       ├── rtc.rs          # WebRTC setup
│       ├── signaling.rs    # Signal server client
│       └── protocol.rs     # Wire protocol
├── signal-server/          # TypeScript signal server
└── scripts/                # Test scripts
    ├── test-tunnel.sh      # Integration test (public signal server)
    └── test-local.sh       # Integration test (local signal server)
```

### Running Tests

```bash
# Test with public signal server (requires internet)
./scripts/test-tunnel.sh

# Test with local signal server (offline)
./scripts/test-local.sh
```

### Protocol Details

Binary framing over WebRTC data channel:
- `HELLO`/`AGREE` — Version negotiation
- `PING`/`PONG` — Keepalive
- `REQ_HEADERS`/`REQ_BODY`/`REQ_END` — HTTP request
- `RES_HEADERS`/`RES_BODY`/`RES_END` — HTTP response

Frame format: `[type:u8][stream_id:u32][payload...]`

Multiple concurrent requests are multiplexed via stream IDs.
