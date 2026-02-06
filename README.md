# P2P HTTP Tunnel over WebRTC

A peer-to-peer HTTP tunneling solution that allows serving HTTP APIs (like OpenAI-compatible LLM endpoints) through WebRTC data channels. Two components:

1. **Signal Server** (TypeScript) — Tiny WebSocket rendezvous server for WebRTC signaling
2. **Tunnel Client** (Rust) — Single binary with `serve` and `proxy` modes

**Public signal server:** `wss://signal-server.fly.dev` (default)

## Architecture

```
┌─────────────┐     WebRTC      ┌─────────────┐
│   Consumer  │◄──────────────►│   Provider  │
│ tunnel proxy│   Data Channel  │ tunnel serve│
│  :8000      │                 │             │
└──────┬──────┘                 └──────┬──────┘
       │                               │
       │ HTTP                          │ HTTP
       ▼                               ▼
  Your Tools                     Upstream API
  (curl, etc)                   (LLM server)

         ┌──────────────┐
         │Signal Server │
         │ fly.dev:443  │
         └──────────────┘
         (only for initial
          handshake)
```

## Quick Start

### Prerequisites

- Rust 1.70+

### 1. Build the Tunnel

```bash
cd tunnel
cargo build --release
```

### 2. Start the Provider (on the machine with the API)

```bash
# Example: expose local Ollama instance
./tunnel/target/release/tunnel serve \
  --room my-secret-room \
  --upstream http://127.0.0.1:11434
```

### 3. Start the Consumer (on your laptop)

```bash
./tunnel/target/release/tunnel proxy \
  --room my-secret-room \
  --listen 127.0.0.1:8000
```

### 4. Use It

```bash
# Now your tools can talk to the remote API as if it's local:
curl http://127.0.0.1:8000/api/tags

# For OpenAI-compatible endpoints:
curl http://127.0.0.1:8000/v1/models
export OPENAI_BASE_URL=http://127.0.0.1:8000
```

> **Note:** The `--signal` flag defaults to `wss://signal-server.fly.dev`. You only need to specify it if running your own signal server.

## CLI Reference

### `tunnel serve`

Exposes an upstream HTTP service through the tunnel.

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--signal` | `TUNNEL_SIGNAL` | `wss://signal-server.fly.dev` | WebSocket URL of signaling server |
| `--room` | `TUNNEL_ROOM` | (required) | Room name for peer discovery |
| `--upstream` | `TUNNEL_UPSTREAM` | (required) | Upstream HTTP URL to forward to |
| `--advertise` | — | `/` | Path prefix to advertise |

### `tunnel proxy`

Creates a local HTTP proxy that tunnels to the remote provider.

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--signal` | `TUNNEL_SIGNAL` | `wss://signal-server.fly.dev` | WebSocket URL of signaling server |
| `--room` | `TUNNEL_ROOM` | (required) | Room name for peer discovery |
| `--listen` | `TUNNEL_LISTEN` | `127.0.0.1:8000` | Local address to listen on |

### `signal-server` (self-hosted)

| Flag | Description |
|------|-------------|
| `--port` | Port to listen on (default: `8787`) |
| `--listen` | Host to bind to (default: `0.0.0.0`) |

## Protocol

The tunnel uses a simple binary framing protocol over WebRTC data channels:

1. **Handshake**: `HELLO` → `AGREE` (version + feature negotiation)
2. **Request**: `REQ_HEADERS` → `REQ_BODY`* → `REQ_END`
3. **Response**: `RES_HEADERS` → `RES_BODY`* → `RES_END`

Each frame: `[type:u8][stream_id:u32][payload...]`

Multiple requests are multiplexed over a single data channel using stream IDs.

## How It Works

1. Both peers connect to the signaling server and join the same room
2. The first peer becomes the **offerer**, the second becomes the **answerer**
3. They exchange SDP offers/answers and ICE candidates through the signaling server
4. Once WebRTC is established, the signaling server is no longer needed
5. The consumer sends a `HELLO` message, the provider responds with `AGREE`
6. HTTP requests from the proxy are framed and sent through the data channel
7. The provider forwards them to the upstream server and streams responses back

## Self-Hosting the Signal Server

The signal server is deployed at `wss://signal-server.fly.dev` and free to use. To run your own:

```bash
cd signal-server
npm install && npm run build
node dist/index.js --port 8787
```

Or deploy to Fly.io:
```bash
cd signal-server
fly launch
fly deploy
```

## Development

### Running the Local Test

```bash
chmod +x test-local.sh
./test-local.sh
```

This starts all components locally and verifies end-to-end tunneling.

### Logging

Set `RUST_LOG` for detailed output:

```bash
RUST_LOG=debug ./tunnel/target/release/tunnel serve ...
```

## Networking Notes

- **LAN/Dev mode**: Works with STUN only (default). Uses mDNS for local discovery.
- **Internet mode**: For NAT traversal across the internet, you may need a TURN server:
  ```
  # Future: --turn turn:your-server:3478 --turn-user user --turn-pass pass
  ```
- The signaling server only relays opaque WebRTC handshake messages — it never sees your HTTP traffic.

## Project Structure

```
├── signal-server/          # TypeScript signaling server
│   ├── src/index.ts        # WebSocket server with room routing
│   ├── package.json
│   └── tsconfig.json
├── tunnel/                 # Rust tunnel client
│   ├── src/
│   │   ├── main.rs         # Entry point, CLI dispatch
│   │   ├── cli.rs          # Clap CLI definitions
│   │   ├── signaling.rs    # WebSocket signaling client
│   │   ├── rtc.rs          # WebRTC peer connection setup
│   │   ├── protocol.rs     # Tunnel framing protocol
│   │   ├── serve.rs        # Provider mode (upstream forwarding)
│   │   └── proxy.rs        # Consumer mode (local HTTP server)
│   └── Cargo.toml
├── test-local.sh           # End-to-end local test
└── README.md
```
