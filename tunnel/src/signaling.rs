use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Messages we send to the signaling server
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum OutgoingSignal {
    #[serde(rename = "join")]
    Join { room: String },
    #[serde(rename = "offer")]
    Offer { sdp: String },
    #[serde(rename = "answer")]
    Answer { sdp: String },
    #[serde(rename = "candidate")]
    Candidate { candidate: String },
    #[serde(rename = "bye")]
    #[allow(dead_code)]
    Bye,
}

/// Messages we receive from the signaling server
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum IncomingSignal {
    Joined {
        #[serde(rename = "peerId")]
        peer_id: String,
        peers: Vec<String>,
    },
    PeerJoined {
        #[serde(rename = "peerId")]
        #[allow(dead_code)]
        peer_id: String,
    },
    Offer {
        #[serde(rename = "peerId")]
        #[allow(dead_code)]
        peer_id: String,
        sdp: String,
    },
    Answer {
        #[serde(rename = "peerId")]
        #[allow(dead_code)]
        peer_id: String,
        sdp: String,
    },
    Candidate {
        #[serde(rename = "peerId")]
        #[allow(dead_code)]
        peer_id: String,
        candidate: String,
    },
    PeerLeft {
        #[serde(rename = "peerId")]
        #[allow(dead_code)]
        peer_id: String,
    },
    Error {
        message: String,
    },
}

pub struct SignalingClient {
    pub send_tx: mpsc::UnboundedSender<OutgoingSignal>,
    pub recv_rx: mpsc::UnboundedReceiver<IncomingSignal>,
}

impl SignalingClient {
    pub async fn connect(signal_url: &str, room: &str) -> Result<Self> {
        info!("connecting to signaling server: {}", signal_url);

        let (ws_stream, _) = connect_async(signal_url)
            .await
            .context("failed to connect to signaling server")?;

        let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();

        // Channel for outgoing messages
        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<OutgoingSignal>();
        // Channel for incoming messages
        let (recv_tx, recv_rx) = mpsc::unbounded_channel::<IncomingSignal>();

        // Send join immediately
        let join_msg = serde_json::to_string(&OutgoingSignal::Join {
            room: room.to_string(),
        })?;
        ws_sink.send(Message::Text(join_msg)).await?;
        info!("sent join for room: {}", room);

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(msg) = send_rx.recv().await {
                let text = match serde_json::to_string(&msg) {
                    Ok(t) => t,
                    Err(e) => {
                        error!("failed to serialize signal message: {}", e);
                        continue;
                    }
                };
                if let Err(e) = ws_sink.send(Message::Text(text)).await {
                    error!("failed to send signal message: {}", e);
                    break;
                }
            }
            debug!("signaling writer task ended");
        });

        // Spawn reader task
        tokio::spawn(async move {
            while let Some(msg_result) = ws_stream_rx.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<IncomingSignal>(&text) {
                            Ok(signal) => {
                                debug!("received signal: {:?}", signal);
                                if recv_tx.send(signal).is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!("failed to parse signal message: {} - {}", e, text);
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        info!("signaling connection closed");
                        break;
                    }
                    Err(e) => {
                        error!("signaling ws error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
            debug!("signaling reader task ended");
        });

        Ok(SignalingClient { send_tx, recv_rx })
    }

    pub fn send(&self, msg: OutgoingSignal) -> Result<()> {
        self.send_tx
            .send(msg)
            .map_err(|_| anyhow!("signaling channel closed"))
    }

    pub async fn recv(&mut self) -> Option<IncomingSignal> {
        self.recv_rx.recv().await
    }
}
