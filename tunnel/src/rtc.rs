use anyhow::{anyhow, Result};
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, error, info, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

use crate::cli::TurnConfig;
use crate::signaling::{IncomingSignal, OutgoingSignal, SignalingClient};

pub struct DataChannelPair {
    pub dc: Arc<RTCDataChannel>,
    pub incoming_rx: mpsc::UnboundedReceiver<Bytes>,
    pub connected: Arc<Notify>,
    pub disconnected: Arc<Notify>,
}

/// Create a WebRTC peer connection with STUN (and optionally TURN)
async fn create_peer_connection(turn_config: &TurnConfig) -> Result<Arc<RTCPeerConnection>> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;

    let mut se = SettingEngine::default();
    // Disable mDNS — it generates unresolvable .local hostnames between processes on macOS.
    // With mDNS disabled, ICE uses direct host IP candidates which work for both LAN and internet.
    se.set_ice_multicast_dns_mode(webrtc::ice::mdns::MulticastDnsMode::Disabled);

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .with_setting_engine(se)
        .build();

    let mut ice_servers = vec![RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        ..Default::default()
    }];

    // Add TURN server if configured
    if let Some(ref turn_url) = turn_config.url {
        info!("adding TURN server: {}", turn_url);
        ice_servers.push(RTCIceServer {
            urls: vec![turn_url.clone()],
            username: turn_config.username.clone().unwrap_or_default(),
            credential: turn_config.password.clone().unwrap_or_default(),
            ..Default::default()
        });
    }

    let config = RTCConfiguration {
        ice_servers,
        ..Default::default()
    };

    let pc = api.new_peer_connection(config).await?;
    Ok(Arc::new(pc))
}

fn setup_data_channel_handlers(
    dc: &Arc<RTCDataChannel>,
    incoming_tx: mpsc::UnboundedSender<Bytes>,
    connected: Arc<Notify>,
) {
    let dc_label = dc.label().to_string();

    let connected_clone = connected.clone();
    dc.on_open(Box::new(move || {
        info!("data channel '{}' opened", dc_label);
        connected_clone.notify_waiters();
        Box::pin(async {})
    }));

    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let data = Bytes::from(msg.data.to_vec());
        let _ = incoming_tx.send(data);
        Box::pin(async {})
    }));

    let dc_label2 = dc.label().to_string();
    dc.on_close(Box::new(move || {
        info!("data channel '{}' closed", dc_label2);
        Box::pin(async {})
    }));
}

/// Apply buffered ICE candidates to the peer connection.
async fn apply_buffered_candidates(
    pc: &RTCPeerConnection,
    buffered: &mut Vec<RTCIceCandidateInit>,
) -> Result<()> {
    if !buffered.is_empty() {
        info!("applying {} buffered ICE candidate(s)", buffered.len());
    }
    for candidate in buffered.drain(..) {
        pc.add_ice_candidate(candidate).await?;
    }
    Ok(())
}

/// Parse an ICE candidate from JSON string, logging errors instead of crashing.
fn parse_ice_candidate(candidate: &str) -> Result<RTCIceCandidateInit> {
    serde_json::from_str(candidate).map_err(|e| anyhow!("invalid ICE candidate JSON: {}", e))
}

/// Check if the data channel is in the Open state.
fn dc_is_open(dc: &RTCDataChannel) -> bool {
    dc.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
}

/// Establish WebRTC connection as the offerer (first peer in room)
pub async fn establish_as_offerer(
    signaling: &mut SignalingClient,
    turn_config: &TurnConfig,
) -> Result<(Arc<RTCPeerConnection>, DataChannelPair)> {
    let pc = create_peer_connection(turn_config).await?;

    // Create data channel
    let dc = pc.create_data_channel("tunnel", None).await?;
    let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
    let connected = Arc::new(Notify::new());

    setup_data_channel_handlers(&dc, incoming_tx, connected.clone());

    // Set up ICE candidate handler
    let send_tx = signaling.send_tx.clone();
    pc.on_ice_candidate(Box::new(move |candidate| {
        let send_tx = send_tx.clone();
        Box::pin(async move {
            if let Some(c) = candidate {
                let json = match c.to_json() {
                    Ok(j) => j,
                    Err(e) => {
                        error!("failed to serialize ICE candidate: {}", e);
                        return;
                    }
                };
                let candidate_str = serde_json::to_string(&json).unwrap();
                debug!("sending ICE candidate");
                let _ = send_tx.send(OutgoingSignal::Candidate {
                    candidate: candidate_str,
                });
            }
        })
    }));

    // Use a Notify to detect when peer connection reaches Connected state
    let pc_connected = Arc::new(Notify::new());
    let pc_connected_clone = pc_connected.clone();
    let pc_failed = Arc::new(Notify::new());
    let pc_failed_clone = pc_failed.clone();
    pc.on_peer_connection_state_change(Box::new(move |state| {
        info!("peer connection state: {:?}", state);
        match state {
            RTCPeerConnectionState::Connected => pc_connected_clone.notify_waiters(),
            RTCPeerConnectionState::Failed => pc_failed_clone.notify_waiters(),
            _ => {}
        }
        Box::pin(async {})
    }));

    // Create offer
    let offer = pc.create_offer(None).await?;
    pc.set_local_description(offer.clone()).await?;

    // Wait for ICE gathering to complete or timeout
    let mut gather_complete = pc.gathering_complete_promise().await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), gather_complete.recv()).await;

    // Send offer with gathered candidates
    let local_desc = pc
        .local_description()
        .await
        .ok_or_else(|| anyhow!("no local description"))?;
    info!("sending offer");
    signaling.send(OutgoingSignal::Offer {
        sdp: local_desc.sdp.clone(),
    })?;

    // Buffer ICE candidates that arrive before we set the remote description
    let mut remote_desc_set = false;
    let mut buffered_candidates: Vec<RTCIceCandidateInit> = Vec::new();

    // Wait for answer and ICE candidates, using select! to also detect connection state changes
    loop {
        tokio::select! {
            signal = signaling.recv() => {
                match signal {
                    Some(IncomingSignal::Answer { sdp, .. }) => {
                        info!("received answer");
                        let answer = RTCSessionDescription::answer(sdp)?;
                        pc.set_remote_description(answer).await?;
                        remote_desc_set = true;
                        apply_buffered_candidates(&pc, &mut buffered_candidates).await?;
                    }
                    Some(IncomingSignal::Candidate { candidate, .. }) => {
                        match parse_ice_candidate(&candidate) {
                            Ok(candidate_init) => {
                                if remote_desc_set {
                                    debug!("applying ICE candidate immediately");
                                    pc.add_ice_candidate(candidate_init).await?;
                                } else {
                                    debug!("buffering ICE candidate (remote description not yet set)");
                                    buffered_candidates.push(candidate_init);
                                }
                            }
                            Err(e) => warn!("skipping bad ICE candidate: {}", e),
                        }
                    }
                    Some(IncomingSignal::PeerLeft { .. }) => {
                        return Err(anyhow!("peer left before connection established"));
                    }
                    Some(IncomingSignal::Error { message }) => {
                        return Err(anyhow!("signaling error: {}", message));
                    }
                    None => {
                        return Err(anyhow!("signaling connection lost"));
                    }
                    other => {
                        debug!("ignoring signal during offer: {:?}", other);
                    }
                }
            }
            _ = pc_connected.notified() => {
                info!("peer connection connected, waiting for data channel...");
                // Give data channel a moment to open
                for _ in 0..50 {
                    if dc_is_open(&dc) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                if dc_is_open(&dc) {
                    break;
                }
                warn!("peer connection connected but data channel not open yet");
            }
            _ = pc_failed.notified() => {
                return Err(anyhow!("ICE connection failed"));
            }
        }

        // Check if data channel is already open
        if dc_is_open(&dc) {
            break;
        }
    }

    info!("WebRTC connection established (offerer)");
    Ok((
        pc,
        DataChannelPair {
            dc,
            incoming_rx,
            connected,
            disconnected: pc_failed,
        },
    ))
}

/// Establish WebRTC connection as the answerer (second peer in room)
pub async fn establish_as_answerer(
    signaling: &mut SignalingClient,
    turn_config: &TurnConfig,
) -> Result<(Arc<RTCPeerConnection>, DataChannelPair)> {
    let pc = create_peer_connection(turn_config).await?;

    let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
    let connected = Arc::new(Notify::new());

    // Set up data channel handler for incoming channels
    let incoming_tx_clone = incoming_tx.clone();
    let connected_clone = connected.clone();
    let dc_holder: Arc<tokio::sync::Mutex<Option<Arc<RTCDataChannel>>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let dc_holder_clone = dc_holder.clone();

    // Notify when data channel is received and opened
    let dc_opened = Arc::new(Notify::new());
    let dc_opened_clone = dc_opened.clone();

    pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        info!("received data channel: {}", dc.label());
        let incoming_tx = incoming_tx_clone.clone();
        let connected = connected_clone.clone();
        let holder = dc_holder_clone.clone();
        let dc_opened = dc_opened_clone.clone();
        Box::pin(async move {
            // Set up open handler to notify
            let dc_opened_inner = dc_opened.clone();
            let dc_label = dc.label().to_string();
            let connected_inner = connected.clone();
            dc.on_open(Box::new(move || {
                info!("data channel '{}' opened", dc_label);
                connected_inner.notify_waiters();
                dc_opened_inner.notify_waiters();
                Box::pin(async {})
            }));

            dc.on_message(Box::new(move |msg: DataChannelMessage| {
                let data = Bytes::from(msg.data.to_vec());
                let _ = incoming_tx.send(data);
                Box::pin(async {})
            }));

            *holder.lock().await = Some(dc);
        })
    }));

    // Set up ICE candidate handler
    let send_tx = signaling.send_tx.clone();
    pc.on_ice_candidate(Box::new(move |candidate| {
        let send_tx = send_tx.clone();
        Box::pin(async move {
            if let Some(c) = candidate {
                let json = match c.to_json() {
                    Ok(j) => j,
                    Err(e) => {
                        error!("failed to serialize ICE candidate: {}", e);
                        return;
                    }
                };
                let candidate_str = serde_json::to_string(&json).unwrap();
                debug!("sending ICE candidate");
                let _ = send_tx.send(OutgoingSignal::Candidate {
                    candidate: candidate_str,
                });
            }
        })
    }));

    // Monitor connection state
    let pc_failed = Arc::new(Notify::new());
    let pc_failed_clone = pc_failed.clone();
    pc.on_peer_connection_state_change(Box::new(move |state| {
        info!("peer connection state: {:?}", state);
        if state == RTCPeerConnectionState::Failed {
            pc_failed_clone.notify_waiters();
        }
        Box::pin(async {})
    }));

    // Buffer ICE candidates that arrive before we set the remote description
    let mut remote_desc_set = false;
    let mut buffered_candidates: Vec<RTCIceCandidateInit> = Vec::new();

    // Wait for offer, then create answer, using select! to detect dc open / failure
    loop {
        tokio::select! {
            signal = signaling.recv() => {
                match signal {
                    Some(IncomingSignal::Offer { sdp, .. }) => {
                        info!("received offer");
                        let offer = RTCSessionDescription::offer(sdp)?;
                        pc.set_remote_description(offer).await?;
                        remote_desc_set = true;
                        apply_buffered_candidates(&pc, &mut buffered_candidates).await?;

                        // Create answer
                        let answer = pc.create_answer(None).await?;
                        pc.set_local_description(answer.clone()).await?;

                        // Wait for ICE gathering
                        let mut gather_complete = pc.gathering_complete_promise().await;
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            gather_complete.recv(),
                        )
                        .await;

                        let local_desc = pc
                            .local_description()
                            .await
                            .ok_or_else(|| anyhow!("no local description"))?;
                        info!("sending answer");
                        signaling.send(OutgoingSignal::Answer {
                            sdp: local_desc.sdp.clone(),
                        })?;
                    }
                    Some(IncomingSignal::Candidate { candidate, .. }) => {
                        match parse_ice_candidate(&candidate) {
                            Ok(candidate_init) => {
                                if remote_desc_set {
                                    debug!("applying ICE candidate immediately");
                                    pc.add_ice_candidate(candidate_init).await?;
                                } else {
                                    debug!("buffering ICE candidate (remote description not yet set)");
                                    buffered_candidates.push(candidate_init);
                                }
                            }
                            Err(e) => warn!("skipping bad ICE candidate: {}", e),
                        }
                    }
                    Some(IncomingSignal::PeerLeft { .. }) => {
                        return Err(anyhow!("peer left before connection established"));
                    }
                    Some(IncomingSignal::Error { message }) => {
                        return Err(anyhow!("signaling error: {}", message));
                    }
                    None => {
                        return Err(anyhow!("signaling connection lost"));
                    }
                    other => {
                        debug!("ignoring signal during answer: {:?}", other);
                    }
                }
            }
            _ = dc_opened.notified() => {
                info!("data channel opened via notification");
                break;
            }
            _ = pc_failed.notified() => {
                return Err(anyhow!("ICE connection failed"));
            }
        }

        // Check if we have a data channel and it's open
        let dc_guard = dc_holder.lock().await;
        if let Some(ref dc) = *dc_guard {
            if dc_is_open(dc) {
                drop(dc_guard);
                break;
            }
        }
        drop(dc_guard);
    }

    // Get the data channel
    let dc_guard = dc_holder.lock().await;
    let dc = dc_guard
        .as_ref()
        .ok_or_else(|| anyhow!("data channel not received"))?
        .clone();
    drop(dc_guard);

    info!("WebRTC connection established (answerer)");
    Ok((
        pc,
        DataChannelPair {
            dc,
            incoming_rx,
            connected,
            disconnected: pc_failed,
        },
    ))
}

/// High-level: connect to signaling, determine role, establish WebRTC
pub async fn connect(
    signal_url: &str,
    room: &str,
    turn_config: &TurnConfig,
) -> Result<(Arc<RTCPeerConnection>, DataChannelPair, SignalingClient)> {
    let mut signaling = SignalingClient::connect(signal_url, room).await?;

    // Wait for joined message to determine our role
    let is_offerer = loop {
        match signaling.recv().await {
            Some(IncomingSignal::Joined { peer_id, peers }) => {
                info!("joined as peer {} - existing peers: {:?}", peer_id, peers);
                if peers.is_empty() {
                    // We're first - wait for peer to join, then we'll be offerer
                    info!("waiting for peer to join room...");
                    match signaling.recv().await {
                        Some(IncomingSignal::PeerJoined { .. }) => {
                            info!("peer joined, we are offerer");
                            break true;
                        }
                        Some(IncomingSignal::Error { message }) => {
                            return Err(anyhow!("signaling error: {}", message));
                        }
                        None => return Err(anyhow!("signaling connection lost")),
                        other => {
                            debug!("ignoring signal while waiting for peer: {:?}", other);
                        }
                    }
                } else {
                    // Peer already in room - we're answerer
                    info!("peer already in room, we are answerer");
                    break false;
                }
            }
            Some(IncomingSignal::Error { message }) => {
                return Err(anyhow!("signaling error: {}", message));
            }
            None => return Err(anyhow!("signaling connection lost")),
            other => {
                debug!("ignoring signal while joining: {:?}", other);
            }
        }
    };

    let (pc, dc_pair) = if is_offerer {
        establish_as_offerer(&mut signaling, turn_config).await?
    } else {
        establish_as_answerer(&mut signaling, turn_config).await?
    };

    Ok((pc, dc_pair, signaling))
}
