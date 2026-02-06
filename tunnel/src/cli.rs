use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "tunnel", about = "P2P HTTP tunnel over WebRTC")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Serve an upstream HTTP service through the tunnel
    Serve {
        /// WebSocket URL of the signaling server
        #[arg(long, env = "TUNNEL_SIGNAL")]
        signal: String,

        /// Room name to join
        #[arg(long, env = "TUNNEL_ROOM")]
        room: String,

        /// Upstream HTTP URL to forward requests to
        #[arg(long, env = "TUNNEL_UPSTREAM")]
        upstream: String,

        /// Path prefix to advertise (e.g. /v1)
        #[arg(long, default_value = "/")]
        advertise: String,

        /// TURN server URL (e.g. turn:turn.example.com:3478)
        #[arg(long, env = "TUNNEL_TURN")]
        turn: Option<String>,

        /// TURN server username
        #[arg(long, env = "TUNNEL_TURN_USER")]
        turn_user: Option<String>,

        /// TURN server password
        #[arg(long, env = "TUNNEL_TURN_PASS")]
        turn_pass: Option<String>,
    },

    /// Create a local HTTP proxy that tunnels to a remote provider
    Proxy {
        /// WebSocket URL of the signaling server
        #[arg(long, env = "TUNNEL_SIGNAL")]
        signal: String,

        /// Room name to join
        #[arg(long, env = "TUNNEL_ROOM")]
        room: String,

        /// Local address to listen on
        #[arg(long, default_value = "127.0.0.1:8000", env = "TUNNEL_LISTEN")]
        listen: String,

        /// TURN server URL (e.g. turn:turn.example.com:3478)
        #[arg(long, env = "TUNNEL_TURN")]
        turn: Option<String>,

        /// TURN server username
        #[arg(long, env = "TUNNEL_TURN_USER")]
        turn_user: Option<String>,

        /// TURN server password
        #[arg(long, env = "TUNNEL_TURN_PASS")]
        turn_pass: Option<String>,
    },
}

/// TURN server configuration extracted from CLI flags.
#[derive(Debug, Clone, Default)]
pub struct TurnConfig {
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}
