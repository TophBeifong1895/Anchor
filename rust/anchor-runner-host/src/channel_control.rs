//! The one client for a channel gateway's private `send` operation.
//!
//! A bot's own assistant node (`channel_tools`) and a delegated `Op.run`
//! (`anchor-channel-send`) both deliver through here, so the platform limits,
//! the request frame and what counts as a confirmed delivery are one rule
//! rather than two copies that could drift apart. The limits mirror the
//! gateway's (a test pins them), so the Host never accepts what the gateway
//! would refuse, without the Host depending on one channel adapter's crate.

use serde_json::{Value, json};
use std::{path::Path, time::Duration};

/// The largest message body the gateway accepts.
const MAX_TEXT_BYTES: usize = 20_480;
/// The largest NDJSON request line the gateway reads.
const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// The longest recipient identifier the gateway accepts.
const MAX_USERID_CHARS: usize = 200;
/// How long a sender waits for the gateway's acknowledgement.
pub(crate) const ACK_TIMEOUT: Duration = Duration::from_secs(20);

const OVER_LIMIT: &str = "message or identifier exceeds platform limits";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SendError {
    /// Refused before anything was written to the gateway; nothing was sent.
    Refused(String),
    /// The gateway answered with an explicit refusal.
    Gateway(String),
    /// The request may have been delivered; the caller must not resend it.
    Uncertain,
}

/// One complete `send` frame, or a refusal before anything is connected.
pub(crate) fn send_frame(
    request_id: &str,
    userid: &str,
    content: &str,
    token: &str,
) -> Result<Vec<u8>, SendError> {
    if content.len() > MAX_TEXT_BYTES || userid.chars().count() > MAX_USERID_CHARS {
        return Err(SendError::Refused(OVER_LIMIT.into()));
    }
    let payload = json!({
        "operation": "send",
        "request_id": request_id,
        "userid": userid,
        "content": content,
        "token": token,
    });
    let mut frame =
        serde_json::to_vec(&payload).map_err(|error| SendError::Refused(error.to_string()))?;
    frame.push(b'\n');
    if frame.len() > MAX_REQUEST_BYTES {
        return Err(SendError::Refused(OVER_LIMIT.into()));
    }
    Ok(frame)
}

/// What one gateway reply line means. A delivery counts only when the gateway
/// accepts this very request: a missing, oversized, malformed or foreign reply
/// could be a lost answer, so it is uncertain and never treated as sent.
pub(crate) fn confirmed(response: &[u8], request_id: &str) -> Result<Value, SendError> {
    if response.is_empty() || response.len() > MAX_REQUEST_BYTES {
        return Err(SendError::Uncertain);
    }
    let value: Value = serde_json::from_slice(response).map_err(|_| SendError::Uncertain)?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(SendError::Gateway(error.to_owned()));
    }
    if value.get("accepted") != Some(&Value::Bool(true))
        || value.get("request_id").and_then(Value::as_str) != Some(request_id)
    {
        return Err(SendError::Uncertain);
    }
    Ok(value)
}

/// Blocking delivery for the `anchor-channel-send` helper, a short-lived
/// process without an async runtime of its own.
pub(crate) fn send_blocking(
    socket: &Path,
    frame: &[u8],
    request_id: &str,
) -> Result<Value, SendError> {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|_| SendError::Refused("gateway is not reachable".into()))?;
    stream
        .set_read_timeout(Some(ACK_TIMEOUT))
        .map_err(|error| SendError::Refused(error.to_string()))?;
    stream.write_all(frame).map_err(|_| SendError::Uncertain)?;
    let mut response = Vec::new();
    BufReader::new(stream)
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_until(b'\n', &mut response)
        .map_err(|_| SendError::Uncertain)?;
    confirmed(&response, request_id)
}

/// Async delivery for the assistant's send tool, over a stream the caller has
/// already connected. The caller bounds the whole exchange by `ACK_TIMEOUT`
/// together with its own cancellation checks.
pub(crate) async fn send_async(
    stream: &mut tokio::net::UnixStream,
    frame: &[u8],
    request_id: &str,
) -> Result<Value, SendError> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    stream
        .write_all(frame)
        .await
        .map_err(|_| SendError::Uncertain)?;
    let mut response = Vec::new();
    BufReader::new(stream)
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_until(b'\n', &mut response)
        .await
        .map_err(|_| SendError::Uncertain)?;
    confirmed(&response, request_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_match_the_gateway() {
        assert_eq!(MAX_TEXT_BYTES, anchor_wecom_gateway::MAX_TEXT_BYTES);
        assert_eq!(MAX_REQUEST_BYTES, anchor_wecom_gateway::MAX_REQUEST_BYTES);
    }

    #[test]
    fn only_this_request_accepted_is_a_delivery() {
        assert!(confirmed(br#"{"accepted":true,"request_id":"r-1"}"#, "r-1").is_ok());
        // A gateway retry answers with the same id and a duplicate flag.
        assert!(
            confirmed(
                br#"{"accepted":true,"request_id":"r-1","duplicate":true}"#,
                "r-1"
            )
            .is_ok()
        );
        for response in [
            &b""[..],
            b"not-json",
            br#"{"accepted":false,"request_id":"r-1"}"#,
            br#"{"accepted":true}"#,
            br#"{"accepted":true,"request_id":"r-2"}"#,
        ] {
            assert_eq!(confirmed(response, "r-1"), Err(SendError::Uncertain));
        }
    }

    #[test]
    fn a_gateway_refusal_keeps_its_reason() {
        assert_eq!(
            confirmed(br#"{"error":"recipient is not allowed"}"#, "r-1"),
            Err(SendError::Gateway("recipient is not allowed".into()))
        );
    }

    #[test]
    fn platform_limits_are_checked_before_connecting() {
        assert!(send_frame("r-1", "alice", &"x".repeat(MAX_TEXT_BYTES), "t").is_ok());
        let long_userid = "u".repeat(MAX_USERID_CHARS + 1);
        for (userid, content) in [
            ("alice", "x".repeat(MAX_TEXT_BYTES + 1)),
            (long_userid.as_str(), "hello".to_owned()),
        ] {
            assert_eq!(
                send_frame("r-1", userid, &content, "t"),
                Err(SendError::Refused(OVER_LIMIT.into()))
            );
        }
    }
}
