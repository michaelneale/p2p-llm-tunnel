use anyhow::Result;
use bytes::Bytes;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, error, info, warn};
use webrtc::data_channel::RTCDataChannel;

use crate::protocol::*;
use crate::rtc::DataChannelPair;

/// Run the serve (provider) side of the tunnel
pub async fn run_serve(
    dc_pair: DataChannelPair,
    upstream_url: String,
    advertise_prefix: String,
) -> Result<()> {
    let DataChannelPair {
        dc,
        mut incoming_rx,
        connected,
    } = dc_pair;

    // Wait for connection
    if dc.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
        info!("data channel already open");
    } else {
        info!("waiting for data channel to be ready...");
        connected.notified().await;
    }
    info!("data channel ready, performing handshake...");

    // Wait for HELLO from consumer (with timeout)
    // Long timeout since peers may connect at very different times
    let hello_msg = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        incoming_rx.recv(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("handshake timeout: no HELLO received within 5 minutes"))?
    .ok_or_else(|| anyhow::anyhow!("data channel closed before handshake"))?;

    let hello_tunnel = TunnelMessage::decode(hello_msg)?;
    if hello_tunnel.msg_type != MessageType::Hello {
        return Err(anyhow::anyhow!(
            "expected HELLO, got {:?}",
            hello_tunnel.msg_type
        ));
    }
    let hello: Hello = serde_json::from_slice(&hello_tunnel.payload)?;
    info!("received HELLO: {:?}", hello);

    let agree =
        Agree::from_hello(&hello).map_err(|e| anyhow::anyhow!("handshake failed: {}", e))?;
    let agree_msg = TunnelMessage::agree(&agree);
    dc.send(&Bytes::from(agree_msg.encode().to_vec())).await?;
    info!("sent AGREE, tunnel ready");

    // Process incoming requests
    let http_client = reqwest::Client::new();

    // Track partial request bodies
    let mut pending_requests: HashMap<u32, (RequestHeaders, Vec<u8>)> = HashMap::new();

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

    while let Some(raw) = incoming_rx.recv().await {
        let msg = match TunnelMessage::decode(raw) {
            Ok(m) => m,
            Err(e) => {
                warn!("failed to decode tunnel message: {}", e);
                continue;
            }
        };

        match msg.msg_type {
            MessageType::ReqHeaders => {
                let headers: RequestHeaders = serde_json::from_slice(&msg.payload)?;
                debug!(
                    "request {} {} {}",
                    headers.stream_id, headers.method, headers.path
                );
                pending_requests.insert(headers.stream_id, (headers, Vec::new()));
            }
            MessageType::ReqBody => {
                if let Some((_, ref mut body)) = pending_requests.get_mut(&msg.stream_id) {
                    body.extend_from_slice(&msg.payload);
                }
            }
            MessageType::ReqEnd => {
                if let Some((headers, body)) = pending_requests.remove(&msg.stream_id) {
                    let dc = dc.clone();
                    let client = http_client.clone();
                    let upstream = upstream_url.clone();
                    let prefix = advertise_prefix.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            handle_request(dc, client, upstream, prefix, headers, body).await
                        {
                            error!("failed to handle request: {}", e);
                        }
                    });
                }
            }
            MessageType::Ping => {
                // Respond with pong
                let pong_msg = TunnelMessage::pong();
                if let Err(e) = dc.send(&Bytes::from(pong_msg.encode().to_vec())).await {
                    warn!("failed to send pong: {}", e);
                } else {
                    debug!("received ping, sent pong");
                }
            }
            MessageType::Pong => {
                debug!("received pong");
            }
            other => {
                debug!("serve ignoring message type {:?}", other);
            }
        }
    }

    ping_task.abort();
    info!("data channel closed, serve ending");
    Ok(())
}

/// Build the upstream URL by stripping the advertise prefix from the request path.
///
/// The advertise prefix is what the provider tells consumers it serves under.
/// When `--advertise /v1` is set, consumers send requests to `/v1/models`,
/// and this function strips `/v1` to forward `/models` to the upstream.
///
/// e.g. upstream="http://localhost:3001", prefix="/v1", path="/v1/models" => "http://localhost:3001/models"
/// With prefix="/" (default), path passes through unchanged.
fn build_upstream_url(upstream_base: &str, advertise_prefix: &str, request_path: &str) -> String {
    let base = upstream_base.trim_end_matches('/');
    let prefix = advertise_prefix.trim_end_matches('/');

    if prefix == "/" || prefix.is_empty() {
        // Default: no prefix transformation
        format!("{}{}", base, request_path)
    } else {
        // Strip the advertise prefix from the request path
        // e.g. prefix="/v1", path="/v1/models" => "/models"
        if let Some(stripped) = request_path.strip_prefix(prefix) {
            let stripped = if stripped.is_empty() { "/" } else { stripped };
            format!("{}{}", base, stripped)
        } else {
            // Path doesn't start with prefix — pass through unchanged
            format!("{}{}", base, request_path)
        }
    }
}

async fn handle_request(
    dc: Arc<RTCDataChannel>,
    client: reqwest::Client,
    upstream_url: String,
    advertise_prefix: String,
    req_headers: RequestHeaders,
    body: Vec<u8>,
) -> Result<()> {
    let stream_id = req_headers.stream_id;
    let url = build_upstream_url(&upstream_url, &advertise_prefix, &req_headers.path);

    debug!(
        "forwarding {} {} -> {}",
        req_headers.method, req_headers.path, url
    );

    let method: reqwest::Method = req_headers.method.parse()?;
    let mut request = client.request(method, &url);

    // Forward headers (skip hop-by-hop headers)
    for (key, value) in &req_headers.headers {
        let lower = key.to_lowercase();
        if lower != "host" && lower != "connection" && lower != "transfer-encoding" {
            request = request.header(key.as_str(), value.as_str());
        }
    }

    if !body.is_empty() {
        request = request.body(body);
    }

    // Execute request
    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            error!("upstream request failed: {}", e);
            // Send 502 response
            let res_headers = ResponseHeaders {
                stream_id,
                status: 502,
                headers: HashMap::from([("content-type".to_string(), "text/plain".to_string())]),
            };
            let msg = TunnelMessage::res_headers(&res_headers);
            dc.send(&Bytes::from(msg.encode().to_vec())).await?;

            let body_msg = TunnelMessage::res_body(
                stream_id,
                Bytes::from(format!("Bad Gateway: {}", e)),
            );
            dc.send(&Bytes::from(body_msg.encode().to_vec())).await?;

            let end_msg = TunnelMessage::res_end(stream_id);
            dc.send(&Bytes::from(end_msg.encode().to_vec())).await?;
            return Ok(());
        }
    };

    // Send response headers
    let status = response.status().as_u16();
    let mut resp_headers = HashMap::new();
    for (key, value) in response.headers() {
        if let Ok(v) = value.to_str() {
            resp_headers.insert(key.to_string(), v.to_string());
        }
    }

    let res_headers = ResponseHeaders {
        stream_id,
        status,
        headers: resp_headers,
    };
    let msg = TunnelMessage::res_headers(&res_headers);
    dc.send(&Bytes::from(msg.encode().to_vec())).await?;

    // Stream response body in chunks as they arrive from upstream.
    // This is critical for SSE/streaming responses (e.g. LLM token streaming).
    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                // The upstream chunk may be larger than our max frame size,
                // so we sub-chunk it.
                let mut offset = 0;
                while offset < chunk.len() {
                    let end = std::cmp::min(offset + MAX_BODY_CHUNK, chunk.len());
                    let sub_chunk = Bytes::copy_from_slice(&chunk[offset..end]);
                    let body_msg = TunnelMessage::res_body(stream_id, sub_chunk);
                    dc.send(&Bytes::from(body_msg.encode().to_vec())).await?;
                    offset = end;
                }
            }
            Err(e) => {
                // Upstream dropped mid-stream — send error frame
                error!("upstream stream error for stream {}: {}", stream_id, e);
                let err_msg = TunnelMessage::error(stream_id, &format!("upstream error: {}", e));
                dc.send(&Bytes::from(err_msg.encode().to_vec())).await?;
                break;
            }
        }
    }

    // Send end
    let end_msg = TunnelMessage::res_end(stream_id);
    dc.send(&Bytes::from(end_msg.encode().to_vec())).await?;

    debug!("response {} complete: status={}", stream_id, status);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_upstream_url_default_prefix() {
        assert_eq!(
            build_upstream_url("http://localhost:3001", "/", "/models"),
            "http://localhost:3001/models"
        );
    }

    #[test]
    fn test_build_upstream_url_with_prefix() {
        // --advertise /v1: consumer sends /v1/models, upstream gets /models
        assert_eq!(
            build_upstream_url("http://localhost:3001", "/v1", "/v1/models"),
            "http://localhost:3001/models"
        );
    }

    #[test]
    fn test_build_upstream_url_trailing_slashes() {
        assert_eq!(
            build_upstream_url("http://localhost:3001/", "/v1/", "/v1/models"),
            "http://localhost:3001/models"
        );
    }

    #[test]
    fn test_build_upstream_url_empty_prefix() {
        assert_eq!(
            build_upstream_url("http://localhost:3001", "", "/chat/completions"),
            "http://localhost:3001/chat/completions"
        );
    }

    #[test]
    fn test_build_upstream_url_exact_prefix() {
        // Consumer sends exactly /v1 with no trailing path
        assert_eq!(
            build_upstream_url("http://localhost:3001", "/v1", "/v1"),
            "http://localhost:3001/"
        );
    }

    #[test]
    fn test_build_upstream_url_no_prefix_match() {
        // Consumer sends path that doesn't start with prefix — pass through
        assert_eq!(
            build_upstream_url("http://localhost:3001", "/v1", "/health"),
            "http://localhost:3001/health"
        );
    }

    #[test]
    fn test_build_upstream_url_nested_prefix() {
        // Deeper prefix stripping
        assert_eq!(
            build_upstream_url("http://localhost:3001", "/api/v1", "/api/v1/chat/completions"),
            "http://localhost:3001/chat/completions"
        );
    }
}
