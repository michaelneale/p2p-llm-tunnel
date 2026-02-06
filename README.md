# P2P HTTP Tunnel over WebRTC

A peer-to-peer HTTP tunneling solution that allows serving HTTP APIs (like OpenAI-compatible LLM endpoints) through WebRTC data channels. Two components:

1. **Signal Server** (TypeScript) — Tiny WebSocket rendezvous server for WebRTC signaling
2. **Tunnel Client** (Rust) — Single binary with `serve` and `proxy` modes

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
         │  ws://:8787  │
         └──────────────┘
         (only for initial
          handshake)
```

## Quick Start

### Prerequisites

- Node.js 18+
- Rust 1.70+

### 1. Build Everything

```bash
# Signal server
cd signal-server
npm install
npm run build
cd ..

# Tunnel client
cd tunnel
cargo build
cd ..
```

### 2. Start the Signal Server

```bash
cd signal-server
node dist/index.js --port 8787
# => ws://0.0.0.0:8787
```

### 3. Start the Provider (on the machine with the API)

```bash
# Assuming your LLM API is running on http://127.0.0.1:3001
./tunnel/target/debug/tunnel serve \
  --signal ws://your-host:8787 \
  --room my-api \
  --upstream http://127.0.0.1:3001
```

### 4. Start the Consumer (on your laptop)

```bash
./tunnel/target/debug/tunnel proxy \
  --signal ws://your-host:8787 \
  --room my-api \
  --listen 127.0.0.1:8000
```

### 5. Use It

```bash
# Now your tools can talk to the remote API as if it's local:
curl http://127.0.0.1:8000/v1/models

# Or set the base URL for OpenAI-compatible tools:
export OPENAI_BASE_URL=http://127.0.0.1:8000
```

## CLI Reference

### `tunnel serve`

Exposes an upstream HTTP service through the tunnel.

| Flag | Env Var | Description |
|------|---------|-------------|
| `--signal` | `TUNNEL_SIGNAL` | WebSocket URL of signaling server |
| `--room` | `TUNNEL_ROOM` | Room name for peer discovery |
| `--upstream` | `TUNNEL_UPSTREAM` | Upstream HTTP URL to forward to |
| `--advertise` | — | Path prefix to advertise (default: `/`) |

### `tunnel proxy`

Creates a local HTTP proxy that tunnels to the remote provider.

| Flag | Env Var | Description |
|------|---------|-------------|
| `--signal` | `TUNNEL_SIGNAL` | WebSocket URL of signaling server |
| `--room` | `TUNNEL_ROOM` | Room name for peer discovery |
| `--listen` | `TUNNEL_LISTEN` | Local address to listen on (default: `127.0.0.1:8000`) |

### `signal-server`

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
RUST_LOG=debug ./tunnel/target/debug/tunnel serve ...
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
