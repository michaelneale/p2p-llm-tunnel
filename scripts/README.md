# Test Scripts

## test-tunnel.sh

Main integration test using the **public signal server** (`wss://signal-server.fly.dev`).

```bash
./scripts/test-tunnel.sh
```

What it does:
1. Starts a local HTTP server with a JSON response
2. Runs `tunnel serve` to expose it
3. Runs `tunnel proxy` to create a local proxy endpoint
4. Curls both direct and through the tunnel to verify

Use this to test real-world behavior across the internet.

## test-local.sh

Integration test using a **local signal server** (for offline/dev testing).

```bash
./scripts/test-local.sh
```

Requires the signal server to be built first:
```bash
cd signal-server && npm install && npm run build
```

Use this when you don't have internet or want to test signal server changes.
