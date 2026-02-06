# Signal Server

A minimal WebSocket signaling server for WebRTC peer discovery. Peers join a "room" and exchange SDP offers/answers and ICE candidates to establish direct connections.

## Public Instance

A public instance is deployed at:
```
wss://signal-server.fly.dev
```

This is the default for the tunnel CLI - you don't need to run your own.

## Running Locally

```bash
npm install
npm run build
node dist/index.js --port 8787
```

Then use `--signal ws://localhost:8787` with the tunnel CLI.

## Deploying to Fly.io

The server is deployed to Fly.io. To deploy your own instance:

### First time setup

```bash
# Login to Fly.io
fly auth login

# Create the app (choose your own name)
fly launch --no-deploy

# Deploy
fly deploy
```

### Subsequent deploys

```bash
fly deploy
```

### Useful commands

```bash
fly status          # Check app status
fly logs            # View logs
fly apps open       # Open in browser (will show "Upgrade Required" - that's normal, it's WebSocket-only)
```

### Configuration

The `fly.toml` file configures:
- **Region**: `syd` (Sydney) - change `primary_region` for your location
- **Auto-scaling**: Scales to zero when idle, starts on first connection
- **Memory**: 256MB (minimal, sufficient for signaling)

## Protocol

Clients connect via WebSocket and send JSON messages:

### Client → Server

```json
{"type": "join", "room": "my-room"}
{"type": "offer", "sdp": "..."}
{"type": "answer", "sdp": "..."}  
{"type": "candidate", "candidate": "..."}
{"type": "bye"}
```

### Server → Client

```json
{"type": "joined", "peerId": "uuid", "peers": ["other-uuid"]}
{"type": "peer-joined", "peerId": "uuid"}
{"type": "offer", "peerId": "uuid", "sdp": "..."}
{"type": "answer", "peerId": "uuid", "sdp": "..."}
{"type": "candidate", "peerId": "uuid", "candidate": "..."}
{"type": "peer-left", "peerId": "uuid"}
{"type": "error", "message": "..."}
```

## Security Notes

- The server only relays WebRTC handshake messages - it never sees actual tunnel traffic
- Room names are the only "authentication" - use unguessable names
- Max 2 peers per room (enforced server-side)
