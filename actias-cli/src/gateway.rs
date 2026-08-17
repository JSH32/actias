//! Client side of the api's live websocket gateway, shared by `actias dev`
//! and `actias tail`.

use colored::Colorize;
use futures::StreamExt;
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{ClientRequestBuilder, Message},
};

use crate::errors::{Error, Result};

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What the gateway answers to requests, and the shape of the log frames it
/// pushes.
#[derive(Deserialize)]
pub struct GatewayReply {
    pub status: String,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    /// Set when `status` is `error` or `log`.
    pub message: Option<String>,
    /// Set when `status` is `log`.
    pub level: Option<String>,
}

/// Connects to the gateway and waits for its `ready` reply.
///
/// The gateway confirms authentication with `ready`; sending earlier races
/// its connection handling and the message would be dropped.
pub async fn connect(ws_url: &str, token: &str) -> Result<WsStream> {
    let request = ClientRequestBuilder::new(
        ws_url
            .parse()
            .map_err(|e| Error::Config(format!("Bad live socket url {ws_url}: {e}")))?,
    )
    .with_header("Authorization", format!("Bearer {token}"));

    let (mut socket, _) = connect_async(request)
        .await
        .map_err(|e| Error::Api(format!("Could not reach the live gateway: {e}")))?;

    let ready = read_reply(&mut socket).await?;
    if ready.status != "ready" {
        return Err(Error::Api(format!(
            "The gateway answered '{}' instead of ready",
            ready.status
        )));
    }

    Ok(socket)
}

/// Waits for the gateway's next json reply, ignoring other frames.
pub async fn read_reply(socket: &mut WsStream) -> Result<GatewayReply> {
    while let Some(message) = socket.next().await {
        match message {
            Ok(Message::Text(text)) => {
                return serde_json::from_str(&text)
                    .map_err(|e| Error::Api(format!("Unreadable gateway reply: {e}")));
            }
            Ok(Message::Close(frame)) => {
                return Err(Error::Api(match frame {
                    Some(frame) => format!("The gateway refused the session: {}", frame.reason),
                    None => "The gateway closed the connection".to_owned(),
                }));
            }
            Ok(_) => {}
            Err(e) => return Err(Error::Api(format!("Live socket failed: {e}"))),
        }
    }

    Err(Error::Api("The gateway closed the connection".to_owned()))
}

/// Wraps a payload in the `{event, data}` envelope the gateway routes on.
pub fn event_message(event: &str, data: serde_json::Value) -> Message {
    Message::text(serde_json::json!({ "event": event, "data": data }).to_string())
}

/// Prints one script log line, level colored by severity.
pub fn print_log_line(reply: &GatewayReply) {
    let level = reply.level.as_deref().unwrap_or("info");
    let label = match level {
        "error" => level.red(),
        "warn" => level.yellow(),
        "debug" => level.bright_black(),
        _ => level.cyan(),
    };

    println!("{:>5} {}", label, reply.message.as_deref().unwrap_or(""));
}

/// Derives the live gateway socket url from the configured api url.
pub fn live_socket_url(api_url: &str) -> Result<String> {
    let base = api_url.trim_end_matches('/');

    let socket_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(Error::Config(format!(
            "The api url '{api_url}' is not http(s), so no live socket url can be derived"
        )));
    };

    // The api serves rest under /api but the websocket at the server root.
    let socket_base = socket_base.trim_end_matches("/api").to_owned();

    Ok(format!("{socket_base}/liveScript"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_url_swaps_scheme_and_drops_the_api_prefix() {
        assert_eq!(
            live_socket_url("http://127.0.0.1:3001/api").unwrap(),
            "ws://127.0.0.1:3001/liveScript"
        );
        assert_eq!(
            live_socket_url("https://api.actias.dev/api/").unwrap(),
            "wss://api.actias.dev/liveScript"
        );
    }

    #[test]
    fn a_non_http_api_url_is_rejected() {
        assert!(live_socket_url("ftp://wat").is_err());
    }
}
