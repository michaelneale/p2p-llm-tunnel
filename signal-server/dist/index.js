"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const ws_1 = require("ws");
const crypto_1 = require("crypto");
// --- State ---
// room name -> Set of peer ids
const rooms = new Map();
// peer id -> Peer
const peers = new Map();
const MAX_ROOM_SIZE = 2; // Only 2 peers per room for a tunnel
// --- Helpers ---
function send(ws, msg) {
    if (ws.readyState === ws_1.WebSocket.OPEN) {
        ws.send(JSON.stringify(msg));
    }
}
function getOtherPeer(peerId, room) {
    const roomPeers = rooms.get(room);
    if (!roomPeers)
        return undefined;
    for (const id of roomPeers) {
        if (id !== peerId) {
            return peers.get(id);
        }
    }
    return undefined;
}
function removePeer(peerId) {
    const peer = peers.get(peerId);
    if (!peer)
        return;
    const roomPeers = rooms.get(peer.room);
    if (roomPeers) {
        roomPeers.delete(peerId);
        if (roomPeers.size === 0) {
            rooms.delete(peer.room);
        }
        else {
            // Notify remaining peer
            for (const id of roomPeers) {
                const other = peers.get(id);
                if (other) {
                    send(other.ws, { type: "peer-left", peerId });
                }
            }
        }
    }
    peers.delete(peerId);
    console.log(`[signal] peer ${peerId} left room ${peer.room}`);
}
// --- Server ---
const host = process.argv.includes("--listen")
    ? process.argv[process.argv.indexOf("--listen") + 1]
    : "0.0.0.0";
const portArg = process.argv.includes("--port")
    ? parseInt(process.argv[process.argv.indexOf("--port") + 1], 10)
    : 8787;
const [listenHost, listenPort] = host.includes(":")
    ? [host.split(":")[0], parseInt(host.split(":")[1], 10)]
    : [host, portArg];
const wss = new ws_1.WebSocketServer({ host: listenHost, port: listenPort });
wss.on("listening", () => {
    console.log(`[signal] listening on ws://${listenHost}:${listenPort}`);
});
wss.on("connection", (ws) => {
    let peerId = null;
    ws.on("message", (data) => {
        let msg;
        try {
            msg = JSON.parse(data.toString());
        }
        catch {
            send(ws, { type: "error", message: "invalid JSON" });
            return;
        }
        switch (msg.type) {
            case "join": {
                if (peerId) {
                    send(ws, { type: "error", message: "already joined a room" });
                    return;
                }
                const room = msg.room;
                if (!room || typeof room !== "string") {
                    send(ws, { type: "error", message: "room name required" });
                    return;
                }
                // Check room capacity
                const roomPeers = rooms.get(room);
                if (roomPeers && roomPeers.size >= MAX_ROOM_SIZE) {
                    send(ws, { type: "error", message: `room '${room}' is full (max ${MAX_ROOM_SIZE})` });
                    return;
                }
                peerId = (0, crypto_1.randomUUID)();
                const peer = { id: peerId, ws, room };
                peers.set(peerId, peer);
                if (!rooms.has(room)) {
                    rooms.set(room, new Set());
                }
                const existingPeers = Array.from(rooms.get(room));
                rooms.get(room).add(peerId);
                console.log(`[signal] peer ${peerId} joined room '${room}' (${rooms.get(room).size}/${MAX_ROOM_SIZE})`);
                // Tell the new peer about existing peers
                send(ws, { type: "joined", peerId, peers: existingPeers });
                // Tell existing peers about the new peer
                for (const existingId of existingPeers) {
                    const existing = peers.get(existingId);
                    if (existing) {
                        send(existing.ws, { type: "peer-joined", peerId });
                    }
                }
                break;
            }
            case "offer": {
                if (!peerId) {
                    send(ws, { type: "error", message: "must join a room first" });
                    return;
                }
                const peer = peers.get(peerId);
                const other = getOtherPeer(peerId, peer.room);
                if (other) {
                    send(other.ws, { type: "offer", peerId, sdp: msg.sdp });
                }
                break;
            }
            case "answer": {
                if (!peerId) {
                    send(ws, { type: "error", message: "must join a room first" });
                    return;
                }
                const peer = peers.get(peerId);
                const other = getOtherPeer(peerId, peer.room);
                if (other) {
                    send(other.ws, { type: "answer", peerId, sdp: msg.sdp });
                }
                break;
            }
            case "candidate": {
                if (!peerId) {
                    send(ws, { type: "error", message: "must join a room first" });
                    return;
                }
                const peer = peers.get(peerId);
                const other = getOtherPeer(peerId, peer.room);
                if (other) {
                    send(other.ws, { type: "candidate", peerId, candidate: msg.candidate });
                }
                break;
            }
            case "bye": {
                if (peerId) {
                    removePeer(peerId);
                    peerId = null;
                }
                break;
            }
            default:
                send(ws, { type: "error", message: `unknown message type` });
        }
    });
    ws.on("close", () => {
        console.log(`[signal] ws closed for peer ${peerId}`);
        if (peerId) {
            removePeer(peerId);
        }
    });
    ws.on("error", (err) => {
        console.error(`[signal] ws error:`, err.message);
        if (peerId) {
            removePeer(peerId);
        }
    });
});
console.log(`[signal] starting signal server...`);
//# sourceMappingURL=index.js.map