use anyhow::Result;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
use webrtc::data_channel::RTCDataChannel;

use crate::protocol::*;
use crate::rtc::DataChannelPair;

/// Sender half for streaming response body chunks to the HTTP handler.
/// The response reader task sends (ResponseHeaders, body_chunk_sender) on first ResHeaders,
/// then streams ResBody chunks through body_chunk_sender, and drops it on ResEnd.
type PendingResponses =
    Arc<Mutex<HashMap<u32, tokio::sync::mpsc::UnboundedSender<StreamEvent>>>>;

/// Events sent from the response reader task to the HTTP handler for a given stream.
#[derive(Debug)]
enum StreamEvent {
    /// Response headers (sent exactly once, first)
    Headers(ResponseHeaders),
    /// A chunk of response body
    Body(Bytes),
    /// An error occurred mid-stream
    Error(String),
    /// End of response
    End,
}

/// Run the proxy (consumer) side of the tunnel
pub async fn run_proxy(
    dc_pair: DataChannelPair,
    listen_addr: String,
) -> Result<()> {
    let DataChannelPair {
        dc,
        mut incoming_rx,
        connected,
    } = dc_pair;

    let tunnel_ready = Arc::new(AtomicBool::new(false));
    let stream_counter = Arc::new(AtomicU32::new(1));
    let pending: PendingResponses = Arc::new(Mutex::new(HashMap::new()));

    // Wait for data channel
    if dc.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
        info!("data channel already open");
    } else {
        info!("waiting for data channel to be ready...");
        connected.notified().await;
    }
    info!("data channel ready, performing handshake...");

    // Send HELLO
    let hello = Hello::new();
    let hello_msg = TunnelMessage::hello(&hello);
    dc.send(&Bytes::from(hello_msg.encode().to_vec())).await?;
    info!("sent HELLO");

    // Wait for AGREE
    // Long timeout since peers may connect at very different times
    let agree_data = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        incoming_rx.recv(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("handshake timeout: no AGREE received within 5 minutes"))?
    .ok_or_else(|| anyhow::anyhow!("data channel closed before handshake"))?;
    let agree_tunnel = TunnelMessage::decode(agree_data)?;
    if agree_tunnel.msg_type != MessageType::Agree {
        return Err(anyhow::anyhow!(
            "expected AGREE, got {:?}",
            agree_tunnel.msg_type
        ));
    }
    let agree: Agree = serde_json::from_slice(&agree_tunnel.payload)?;
    info!("received AGREE: {:?}", agree);
    tunnel_ready.store(true, Ordering::SeqCst);

    // Keepalive: send ping every 10 seconds
    let dc_ping = dc.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            let ping_msg = TunnelMessage::ping();
            if dc_ping.send(&Bytes::from(ping_msg.encode().to_vec())).await.is_err() {
                debug!("ping send failed, channel likely closed");
                break;
            }
            debug!("sent keepalive ping");
        }
    });

    // Spawn response reader task — routes incoming tunnel messages to per-stream channels
    let pending_clone = pending.clone();
    let dc_for_pong = dc.clone();
    tokio::spawn(async move {
        while let Some(raw) = incoming_rx.recv().await {
            let msg = match TunnelMessage::decode(raw) {
                Ok(m) => m,
                Err(e) => {
                    warn!("failed to decode tunnel message: {}", e);
                    continue;
                }
            };

            match msg.msg_type {
                MessageType::ResHeaders => {
                    let headers: ResponseHeaders = match serde_json::from_slice(&msg.payload) {
                        Ok(h) => h,
                        Err(e) => {
                            error!("failed to parse response headers: {}", e);
                            continue;
                        }
                    };
                    debug!("response headers for stream {}: status={}", headers.stream_id, headers.status);
                    let pending_guard = pending_clone.lock().await;
                    if let Some(tx) = pending_guard.get(&headers.stream_id) {
                        let _ = tx.send(StreamEvent::Headers(headers));
                    }
                }
                MessageType::ResBody => {
                    let pending_guard = pending_clone.lock().await;
                    if let Some(tx) = pending_guard.get(&msg.stream_id) {
                        let _ = tx.send(StreamEvent::Body(msg.payload));
                    }
                }
                MessageType::ResEnd => {
                    let mut pending_guard = pending_clone.lock().await;
                    if let Some(tx) = pending_guard.remove(&msg.stream_id) {
                        let _ = tx.send(StreamEvent::End);
                        // tx is dropped here, closing the channel
                    }
                }
                MessageType::Error => {
                    let err_msg = String::from_utf8_lossy(&msg.payload).to_string();
                    error!("tunnel error for stream {}: {}", msg.stream_id, err_msg);
                    let mut pending_guard = pending_clone.lock().await;
                    if let Some(tx) = pending_guard.remove(&msg.stream_id) {
                        let _ = tx.send(StreamEvent::Error(err_msg));
                    }
                }
                MessageType::Ping => {
                    // Respond with pong
                    let pong_msg = TunnelMessage::pong();
                    if let Err(e) = dc_for_pong.send(&Bytes::from(pong_msg.encode().to_vec())).await {
                        warn!("failed to send pong: {}", e);
                    } else {
                        debug!("received ping, sent pong");
                    }
                }
                MessageType::Pong => {
                    debug!("received pong");
                }
                other => {
                    debug!("proxy ignoring message type {:?}", other);
                }
            }
        }
        debug!("response reader task ended");
    });

    // Start HTTP server
    let addr: SocketAddr = listen_addr.parse()?;
    let listener = TcpListener::bind(addr).await?;
    info!("proxy listening on http://{}", addr);

    loop {
        let (stream, remote_addr) = listener.accept().await?;
        let io = hyper_util::rt::TokioIo::new(stream);

        let dc = dc.clone();
        let tunnel_ready = tunnel_ready.clone();
        let stream_counter = stream_counter.clone();
        let pending = pending.clone();

        tokio::spawn(async move {
            let dc = dc.clone();
            let tunnel_ready = tunnel_ready.clone();
            let stream_counter = stream_counter.clone();
            let pending = pending.clone();

            let service = service_fn(move |req: Request<Incoming>| {
                let dc = dc.clone();
                let tunnel_ready = tunnel_ready.clone();
                let stream_counter = stream_counter.clone();
                let pending = pending.clone();
                async move {
                    handle_proxy_request(req, dc, tunnel_ready, stream_counter, pending).await
                }
            });

            if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                if !e.to_string().contains("connection closed") {
                    error!("HTTP connection error from {}: {}", remote_addr, e);
                }
            }
        });
    }
}

/// A streaming response body that receives chunks from the tunnel via an mpsc channel.
struct StreamingBody {
    rx: tokio::sync::mpsc::UnboundedReceiver<Bytes>,
}

impl hyper::body::Body for StreamingBody {
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match self.rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(chunk)) => {
                std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(chunk))))
            }
            std::task::Poll::Ready(None) => {
                // Channel closed — body complete
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

async fn handle_proxy_request(
    req: Request<Incoming>,
    dc: Arc<RTCDataChannel>,
    tunnel_ready: Arc<AtomicBool>,
    stream_counter: Arc<AtomicU32>,
    pending: PendingResponses,
) -> Result<Response<http_body_util::Either<http_body_util::Full<Bytes>, StreamingBody>>, hyper::Error> {
    // Check if tunnel is ready
    if !tunnel_ready.load(Ordering::SeqCst) {
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("content-type", "text/plain")
            .body(http_body_util::Either::Left(http_body_util::Full::new(Bytes::from("Tunnel not ready"))))
            .unwrap());
    }

    let stream_id = stream_counter.fetch_add(1, Ordering::SeqCst);
    let method = req.method().to_string();
    let path = req.uri().path_and_query().map(|pq| pq.to_string()).unwrap_or_else(|| "/".to_string());

    // Collect headers
    let mut headers = HashMap::new();
    for (key, value) in req.headers() {
        if let Ok(v) = value.to_str() {
            headers.insert(key.to_string(), v.to_string());
        }
    }

    debug!("proxying {} {} (stream {})", method, path, stream_id);

    // Collect body
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            error!("failed to read request body: {}", e);
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(http_body_util::Either::Left(http_body_util::Full::new(Bytes::from("Failed to read body"))))
                .unwrap());
        }
    };

    // Create stream event channel for this request
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    {
        let mut pending_guard = pending.lock().await;
        pending_guard.insert(stream_id, event_tx);
    }

    // Send request through tunnel
    let req_headers = RequestHeaders {
        stream_id,
        method,
        path,
        headers,
    };

    let headers_msg = TunnelMessage::req_headers(&req_headers);
    if let Err(e) = dc.send(&Bytes::from(headers_msg.encode().to_vec())).await {
        error!("failed to send request headers: {}", e);
        let mut pending_guard = pending.lock().await;
        pending_guard.remove(&stream_id);
        return Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(http_body_util::Either::Left(http_body_util::Full::new(Bytes::from("Tunnel send failed"))))
            .unwrap());
    }

    // Send body if present
    if !body.is_empty() {
        let mut offset = 0;
        while offset < body.len() {
            let end = std::cmp::min(offset + MAX_BODY_CHUNK, body.len());
            let chunk = Bytes::copy_from_slice(&body[offset..end]);
            let body_msg = TunnelMessage::req_body(stream_id, chunk);
            if let Err(e) = dc.send(&Bytes::from(body_msg.encode().to_vec())).await {
                error!("failed to send request body: {}", e);
                break;
            }
            offset = end;
        }
    }

    // Send end
    let end_msg = TunnelMessage::req_end(stream_id);
    if let Err(e) = dc.send(&Bytes::from(end_msg.encode().to_vec())).await {
        error!("failed to send request end: {}", e);
    }

    // Wait for response headers (with timeout)
    let res_headers = match tokio::time::timeout(
        std::time::Duration::from_secs(60),
        async {
            while let Some(event) = event_rx.recv().await {
                match event {
                    StreamEvent::Headers(h) => return Ok(h),
                    StreamEvent::Error(e) => return Err(e),
                    StreamEvent::End => return Err("response ended before headers".to_string()),
                    StreamEvent::Body(_) => {
                        warn!("received body chunk before headers for stream {}", stream_id);
                    }
                }
            }
            Err("stream channel closed before headers".to_string())
        },
    )
    .await
    {
        Ok(Ok(headers)) => headers,
        Ok(Err(e)) => {
            let mut pending_guard = pending.lock().await;
            pending_guard.remove(&stream_id);
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/plain")
                .body(http_body_util::Either::Left(http_body_util::Full::new(Bytes::from(format!("Tunnel error: {}", e)))))
                .unwrap());
        }
        Err(_) => {
            // Timeout
            let mut pending_guard = pending.lock().await;
            pending_guard.remove(&stream_id);
            return Ok(Response::builder()
                .status(StatusCode::GATEWAY_TIMEOUT)
                .body(http_body_util::Either::Left(http_body_util::Full::new(Bytes::from("Tunnel response timeout"))))
                .unwrap());
        }
    };

    // Build streaming response
    let status = StatusCode::from_u16(res_headers.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::builder().status(status);

    for (key, value) in &res_headers.headers {
        // Skip hop-by-hop headers
        let lower = key.to_lowercase();
        if lower != "transfer-encoding" && lower != "connection" {
            response = response.header(key.as_str(), value.as_str());
        }
    }

    // Create a body chunk channel — the response reader task will feed chunks into it
    let (body_tx, body_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();

    // Spawn a task that reads remaining stream events and forwards body chunks
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                StreamEvent::Body(chunk) => {
                    if body_tx.send(chunk).is_err() {
                        // HTTP client disconnected
                        debug!("HTTP client disconnected for stream {}", stream_id);
                        break;
                    }
                }
                StreamEvent::End => {
                    // Drop body_tx to signal end of body
                    break;
                }
                StreamEvent::Error(e) => {
                    warn!("tunnel error mid-stream for {}: {}", stream_id, e);
                    // Drop body_tx to terminate the stream
                    break;
                }
                StreamEvent::Headers(_) => {
                    warn!("unexpected duplicate headers for stream {}", stream_id);
                }
            }
        }
        // body_tx is dropped here, closing the body stream
    });

    let streaming_body = StreamingBody { rx: body_rx };

    Ok(response
        .body(http_body_util::Either::Right(streaming_body))
        .unwrap())
}
