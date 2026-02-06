use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current protocol version
pub const PROTOCOL_VERSION: u32 = 1;
pub const PROTOCOL_NAME: &str = "httptunnel";

/// Maximum frame size (64KB to stay within WebRTC data channel limits)
pub const MAX_FRAME_SIZE: usize = 64 * 1024;
/// Max body chunk size (leave room for header)
pub const MAX_BODY_CHUNK: usize = MAX_FRAME_SIZE - 128;

// --- Handshake messages ---

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Hello {
    pub proto: String,
    pub min_version: u32,
    pub max_version: u32,
    pub features: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Agree {
    pub version: u32,
    pub features: Vec<String>,
}

impl Hello {
    pub fn new() -> Self {
        Hello {
            proto: PROTOCOL_NAME.to_string(),
            min_version: 1,
            max_version: PROTOCOL_VERSION,
            features: vec!["sse".to_string()],
        }
    }
}

impl Agree {
    /// Negotiate version and features from a HELLO message.
    /// Picks the highest version both sides support.
    pub fn from_hello(hello: &Hello) -> Result<Self, String> {
        if hello.proto != PROTOCOL_NAME {
            return Err(format!("unknown protocol: {}", hello.proto));
        }

        // Our supported range
        let our_min: u32 = 1;
        let our_max: u32 = PROTOCOL_VERSION;

        // Find overlap: max of mins .. min of maxes
        let overlap_min = hello.min_version.max(our_min);
        let overlap_max = hello.max_version.min(our_max);

        if overlap_min > overlap_max {
            return Err(format!(
                "no compatible version: peer=[{},{}], ours=[{},{}]",
                hello.min_version, hello.max_version, our_min, our_max
            ));
        }

        // Pick the highest compatible version
        let agreed_version = overlap_max;

        // Intersect features
        let our_features: Vec<String> = vec!["sse".to_string()];
        let agreed: Vec<String> = hello
            .features
            .iter()
            .filter(|f| our_features.contains(f))
            .cloned()
            .collect();

        Ok(Agree {
            version: agreed_version,
            features: agreed,
        })
    }
}

// --- Tunnel message types ---

/// Message type tags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Hello = 1,
    Agree = 2,
    ReqHeaders = 10,
    ReqBody = 11,
    ReqEnd = 12,
    ResHeaders = 20,
    ResBody = 21,
    ResEnd = 22,
    Error = 99,
}

impl MessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Hello),
            2 => Some(Self::Agree),
            10 => Some(Self::ReqHeaders),
            11 => Some(Self::ReqBody),
            12 => Some(Self::ReqEnd),
            20 => Some(Self::ResHeaders),
            21 => Some(Self::ResBody),
            22 => Some(Self::ResEnd),
            99 => Some(Self::Error),
            _ => None,
        }
    }
}

/// HTTP request metadata sent over the tunnel
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RequestHeaders {
    pub stream_id: u32,
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
}

/// HTTP response metadata sent over the tunnel
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseHeaders {
    pub stream_id: u32,
    pub status: u16,
    pub headers: HashMap<String, String>,
}

/// A framed tunnel message
#[derive(Debug, Clone)]
pub struct TunnelMessage {
    pub msg_type: MessageType,
    pub stream_id: u32,
    pub payload: Bytes,
}

impl TunnelMessage {
    /// Encode: [1 byte type][4 byte stream_id][payload...]
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(5 + self.payload.len());
        buf.put_u8(self.msg_type as u8);
        buf.put_u32(self.stream_id);
        buf.put_slice(&self.payload);
        buf.freeze()
    }

    /// Decode from raw bytes
    pub fn decode(mut data: Bytes) -> Result<Self, anyhow::Error> {
        if data.len() < 5 {
            return Err(anyhow::anyhow!("message too short: {} bytes", data.len()));
        }
        let type_byte = data[0];
        let msg_type = MessageType::from_u8(type_byte)
            .ok_or_else(|| anyhow::anyhow!("unknown message type: {}", type_byte))?;
        data.advance(1);
        let stream_id = (&data[..4]).get_u32();
        data.advance(4);
        Ok(TunnelMessage {
            msg_type,
            stream_id,
            payload: data,
        })
    }

    // Convenience constructors

    pub fn hello(hello: &Hello) -> Self {
        TunnelMessage {
            msg_type: MessageType::Hello,
            stream_id: 0,
            payload: Bytes::from(serde_json::to_vec(hello).unwrap()),
        }
    }

    pub fn agree(agree: &Agree) -> Self {
        TunnelMessage {
            msg_type: MessageType::Agree,
            stream_id: 0,
            payload: Bytes::from(serde_json::to_vec(agree).unwrap()),
        }
    }

    pub fn req_headers(headers: &RequestHeaders) -> Self {
        TunnelMessage {
            msg_type: MessageType::ReqHeaders,
            stream_id: headers.stream_id,
            payload: Bytes::from(serde_json::to_vec(headers).unwrap()),
        }
    }

    pub fn req_body(stream_id: u32, data: Bytes) -> Self {
        TunnelMessage {
            msg_type: MessageType::ReqBody,
            stream_id,
            payload: data,
        }
    }

    pub fn req_end(stream_id: u32) -> Self {
        TunnelMessage {
            msg_type: MessageType::ReqEnd,
            stream_id,
            payload: Bytes::new(),
        }
    }

    pub fn res_headers(headers: &ResponseHeaders) -> Self {
        TunnelMessage {
            msg_type: MessageType::ResHeaders,
            stream_id: headers.stream_id,
            payload: Bytes::from(serde_json::to_vec(headers).unwrap()),
        }
    }

    pub fn res_body(stream_id: u32, data: Bytes) -> Self {
        TunnelMessage {
            msg_type: MessageType::ResBody,
            stream_id,
            payload: data,
        }
    }

    pub fn res_end(stream_id: u32) -> Self {
        TunnelMessage {
            msg_type: MessageType::ResEnd,
            stream_id,
            payload: Bytes::new(),
        }
    }

    pub fn error(stream_id: u32, msg: &str) -> Self {
        TunnelMessage {
            msg_type: MessageType::Error,
            stream_id,
            payload: Bytes::from(msg.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Encode/Decode roundtrip tests ---

    #[test]
    fn test_roundtrip_hello() {
        let hello = Hello::new();
        let msg = TunnelMessage::hello(&hello);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::Hello);
        assert_eq!(decoded.stream_id, 0);
        let parsed: Hello = serde_json::from_slice(&decoded.payload).unwrap();
        assert_eq!(parsed.proto, PROTOCOL_NAME);
        assert_eq!(parsed.min_version, 1);
        assert_eq!(parsed.max_version, PROTOCOL_VERSION);
    }

    #[test]
    fn test_roundtrip_agree() {
        let agree = Agree {
            version: 1,
            features: vec!["sse".to_string()],
        };
        let msg = TunnelMessage::agree(&agree);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::Agree);
        let parsed: Agree = serde_json::from_slice(&decoded.payload).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.features, vec!["sse"]);
    }

    #[test]
    fn test_roundtrip_req_headers() {
        let headers = RequestHeaders {
            stream_id: 42,
            method: "POST".to_string(),
            path: "/v1/chat/completions".to_string(),
            headers: HashMap::from([
                ("content-type".to_string(), "application/json".to_string()),
            ]),
        };
        let msg = TunnelMessage::req_headers(&headers);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::ReqHeaders);
        assert_eq!(decoded.stream_id, 42);
        let parsed: RequestHeaders = serde_json::from_slice(&decoded.payload).unwrap();
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.path, "/v1/chat/completions");
    }

    #[test]
    fn test_roundtrip_req_body() {
        let body = Bytes::from("hello world");
        let msg = TunnelMessage::req_body(7, body.clone());
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::ReqBody);
        assert_eq!(decoded.stream_id, 7);
        assert_eq!(decoded.payload, body);
    }

    #[test]
    fn test_roundtrip_req_end() {
        let msg = TunnelMessage::req_end(7);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::ReqEnd);
        assert_eq!(decoded.stream_id, 7);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn test_roundtrip_res_headers() {
        let headers = ResponseHeaders {
            stream_id: 99,
            status: 200,
            headers: HashMap::from([
                ("content-type".to_string(), "text/event-stream".to_string()),
            ]),
        };
        let msg = TunnelMessage::res_headers(&headers);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::ResHeaders);
        assert_eq!(decoded.stream_id, 99);
        let parsed: ResponseHeaders = serde_json::from_slice(&decoded.payload).unwrap();
        assert_eq!(parsed.status, 200);
    }

    #[test]
    fn test_roundtrip_res_body() {
        let data = Bytes::from("data: {\"token\": \"hello\"}\n\n");
        let msg = TunnelMessage::res_body(99, data.clone());
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::ResBody);
        assert_eq!(decoded.stream_id, 99);
        assert_eq!(decoded.payload, data);
    }

    #[test]
    fn test_roundtrip_res_end() {
        let msg = TunnelMessage::res_end(99);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::ResEnd);
        assert_eq!(decoded.stream_id, 99);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn test_roundtrip_error() {
        let msg = TunnelMessage::error(5, "upstream timeout");
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::Error);
        assert_eq!(decoded.stream_id, 5);
        assert_eq!(&decoded.payload[..], b"upstream timeout");
    }

    // --- Negative: corrupt/truncated messages ---

    #[test]
    fn test_decode_empty() {
        let result = TunnelMessage::decode(Bytes::new());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn test_decode_too_short() {
        let result = TunnelMessage::decode(Bytes::from_static(&[1, 0, 0]));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn test_decode_unknown_type() {
        let result = TunnelMessage::decode(Bytes::from_static(&[255, 0, 0, 0, 0]));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown message type"));
    }

    #[test]
    fn test_decode_exactly_header_no_payload() {
        // 5 bytes: type=ReqEnd(12), stream_id=0 — valid, empty payload
        let result = TunnelMessage::decode(Bytes::from_static(&[12, 0, 0, 0, 0]));
        assert!(result.is_ok());
        let msg = result.unwrap();
        assert_eq!(msg.msg_type, MessageType::ReqEnd);
        assert_eq!(msg.stream_id, 0);
        assert!(msg.payload.is_empty());
    }

    // --- Boundary: edge cases ---

    #[test]
    fn test_zero_stream_id() {
        let msg = TunnelMessage::req_end(0);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.stream_id, 0);
    }

    #[test]
    fn test_max_stream_id() {
        let msg = TunnelMessage::req_end(u32::MAX);
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.stream_id, u32::MAX);
    }

    #[test]
    fn test_large_payload() {
        let data = Bytes::from(vec![0xABu8; MAX_BODY_CHUNK]);
        let msg = TunnelMessage::res_body(1, data.clone());
        let encoded = msg.encode();
        assert_eq!(encoded.len(), 5 + MAX_BODY_CHUNK);
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.payload.len(), MAX_BODY_CHUNK);
        assert_eq!(decoded.payload, data);
    }

    #[test]
    fn test_empty_payload_body() {
        let msg = TunnelMessage::res_body(1, Bytes::new());
        let encoded = msg.encode();
        let decoded = TunnelMessage::decode(encoded).unwrap();
        assert_eq!(decoded.msg_type, MessageType::ResBody);
        assert!(decoded.payload.is_empty());
    }

    // --- Version negotiation tests ---

    #[test]
    fn test_version_negotiation_exact_match() {
        let hello = Hello {
            proto: PROTOCOL_NAME.to_string(),
            min_version: 1,
            max_version: 1,
            features: vec!["sse".to_string()],
        };
        let agree = Agree::from_hello(&hello).unwrap();
        assert_eq!(agree.version, 1);
    }

    #[test]
    fn test_version_negotiation_range_overlap() {
        // Peer supports 1-3, we support 1 (PROTOCOL_VERSION=1)
        // Overlap is [1,1], pick highest = 1
        let hello = Hello {
            proto: PROTOCOL_NAME.to_string(),
            min_version: 1,
            max_version: 3,
            features: vec!["sse".to_string()],
        };
        let agree = Agree::from_hello(&hello).unwrap();
        assert_eq!(agree.version, 1);
    }

    #[test]
    fn test_version_negotiation_no_overlap() {
        // Peer supports 5-10, we support 1
        let hello = Hello {
            proto: PROTOCOL_NAME.to_string(),
            min_version: 5,
            max_version: 10,
            features: vec![],
        };
        let result = Agree::from_hello(&hello);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no compatible version"));
    }

    #[test]
    fn test_version_negotiation_wrong_protocol() {
        let hello = Hello {
            proto: "wrongproto".to_string(),
            min_version: 1,
            max_version: 1,
            features: vec![],
        };
        let result = Agree::from_hello(&hello);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown protocol"));
    }

    #[test]
    fn test_feature_intersection() {
        let hello = Hello {
            proto: PROTOCOL_NAME.to_string(),
            min_version: 1,
            max_version: 1,
            features: vec!["sse".to_string(), "gzip".to_string(), "unknown".to_string()],
        };
        let agree = Agree::from_hello(&hello).unwrap();
        // We only support "sse", so only that should be agreed
        assert_eq!(agree.features, vec!["sse"]);
    }

    #[test]
    fn test_feature_no_overlap() {
        let hello = Hello {
            proto: PROTOCOL_NAME.to_string(),
            min_version: 1,
            max_version: 1,
            features: vec!["gzip".to_string(), "brotli".to_string()],
        };
        let agree = Agree::from_hello(&hello).unwrap();
        assert!(agree.features.is_empty());
    }

    #[test]
    fn test_hello_default_values() {
        let hello = Hello::new();
        assert_eq!(hello.proto, PROTOCOL_NAME);
        assert_eq!(hello.min_version, 1);
        assert_eq!(hello.max_version, PROTOCOL_VERSION);
        assert!(hello.features.contains(&"sse".to_string()));
    }
}
