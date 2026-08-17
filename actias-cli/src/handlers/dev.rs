//! Live development: watch a project directory and mirror every save into a
//! live session the worker serves at `/_live/<identifier>/<session>/`.

use std::{path::Path, pin::Pin, time::Duration};

use colored::Colorize;
use futures::{SinkExt, StreamExt};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::{net::TcpStream, sync::mpsc};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{ClientRequestBuilder, Message},
};

use crate::{
    client::Client,
    errors::{Error, Result, progenitor_error},
    script::ScriptConfig,
    settings::Settings,
    util::get_dir,
};

/// How long a burst of file events settles before one update is sent.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Keeps an idle session alive; must stay well under the server's session
/// ttl, which is two minutes.
const HEARTBEAT: Duration = Duration::from_secs(30);

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What the gateway answers to `start`, `update` and `ping`, and the shape
/// of the log frames it pushes.
#[derive(Deserialize)]
struct GatewayReply {
    status: String,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    /// Set when `status` is `error` or `log`.
    message: Option<String>,
    /// Set when `status` is `log`.
    level: Option<String>,
}

/// How one connected session ended, deciding whether to reconnect.
enum SessionEnd {
    /// The user asked to stop; the session is left to expire on its ttl.
    Quit,
    /// The connection dropped; a fresh session should replace it.
    Disconnected,
}

/// Handle dev command
pub async fn handle(
    client: &Client,
    settings: &Settings,
    directory: &str,
    worker_url: &str,
) -> Result<()> {
    let script_path = get_dir(directory, false, false).map_err(Error::Io)?;
    let config = ScriptConfig::from_path(&script_path).map_err(Error::Script)?;

    let Some(script_id) = config.id.clone() else {
        return Err(Error::Script(format!(
            "This project has no script ID yet; run {} once to create it.",
            "actias publish".yellow()
        )));
    };

    let script = client
        .get_script()
        .id(&script_id)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();

    // One watcher outlives every reconnect, so changes made while offline
    // still trigger an update as soon as a session is back.
    let (fs_tx, mut fs_rx) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !matches!(event.kind, EventKind::Access(_))
        {
            let _ = fs_tx.send(());
        }
    })
    .map_err(|e| Error::Io(format!("Failed to start the file watcher: {e}")))?;
    watcher
        .watch(&script_path, RecursiveMode::Recursive)
        .map_err(|e| Error::Io(format!("Failed to watch {}: {e}", script_path.display())))?;

    let ws_url = live_socket_url(&settings.api_url)?;

    println!(
        "👀 Watching {} for {}",
        script_path.display().to_string().purple(),
        script.public_identifier.purple(),
    );

    let mut backoff = Duration::from_secs(1);
    loop {
        let end = run_session(
            &ws_url,
            &settings.token,
            &script_path,
            &script_id,
            &script.public_identifier,
            worker_url,
            &mut fs_rx,
        )
        .await;

        match end {
            Ok(SessionEnd::Quit) => {
                println!("👋 Session left to expire; bye.");
                return Ok(());
            }
            Ok(SessionEnd::Disconnected) => {
                println!("{}", "🔌 Connection lost, reconnecting...".yellow());
                backoff = Duration::from_secs(1);
            }
            Err(error) => {
                println!(
                    "{} {error} (retrying in {}s)",
                    "⚠️".yellow(),
                    backoff.as_secs()
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

/// Runs one connected session until the user quits or the socket drops.
async fn run_session(
    ws_url: &str,
    token: &str,
    script_path: &Path,
    script_id: &str,
    identifier: &str,
    worker_url: &str,
    fs_rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<SessionEnd> {
    let request = ClientRequestBuilder::new(
        ws_url
            .parse()
            .map_err(|e| Error::Config(format!("Bad live socket url {ws_url}: {e}")))?,
    )
    .with_header("Authorization", format!("Bearer {token}"));

    let (mut socket, _) = connect_async(request)
        .await
        .map_err(|e| Error::Api(format!("Could not reach the live gateway: {e}")))?;

    // The gateway confirms authentication with `ready`; sending earlier
    // races its connection handling and the message would be dropped.
    let ready = read_reply(&mut socket).await?;
    if ready.status != "ready" {
        return Err(Error::Api(format!(
            "The gateway answered '{}' instead of ready",
            ready.status
        )));
    }

    // Starting sends the full working tree, so the session serves the state
    // on disk right now, not the last published revision.
    socket
        .send(event_message(
            "start",
            live_payload(script_path, script_id, None)?,
        ))
        .await
        .map_err(|e| Error::Api(format!("Failed to start the session: {e}")))?;

    let reply = read_reply(&mut socket).await?;
    let Some(session_id) = reply.session_id else {
        return Err(Error::Api(format!(
            "The gateway answered '{}{}' without a session id",
            reply.status,
            reply.message.map(|m| format!(": {m}")).unwrap_or_default()
        )));
    };

    println!(
        "🔴 Live at {}",
        format!(
            "{}/_live/{}/{}/",
            worker_url.trim_end_matches('/'),
            identifier,
            session_id
        )
        .purple()
    );

    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.reset();

    // A settling timer only exists while a burst of file events is pending,
    // so one update goes out per save, not one per event.
    let mut settling: Option<Pin<Box<tokio::time::Sleep>>> = None;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = socket.close(None).await;
                return Ok(SessionEnd::Quit);
            }

            Some(()) = fs_rx.recv() => {
                settling = Some(Box::pin(tokio::time::sleep(DEBOUNCE)));
            }

            () = async { settling.as_mut().expect("armed by the guard").await }, if settling.is_some() => {
                settling = None;

                match live_payload(script_path, script_id, Some(&session_id)) {
                    Ok(payload) => {
                        socket
                            .send(event_message("update", payload))
                            .await
                            .map_err(|e| Error::Api(format!("Failed to send the update: {e}")))?;
                        println!("📤 Updated {}", chrono::Local::now().format("%H:%M:%S").to_string().bright_black());
                    }
                    // A save can be observed mid-write or with a broken
                    // config; report it and keep watching rather than dying.
                    Err(error) => println!("{} {error}", "⚠️".yellow()),
                }
            }

            _ = heartbeat.tick() => {
                socket
                    .send(event_message("ping", serde_json::Value::Null))
                    .await
                    .map_err(|e| Error::Api(format!("Failed to send the heartbeat: {e}")))?;
            }

            message = socket.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<GatewayReply>(&text) {
                            Ok(reply) if reply.status == "log" => print_log_line(&reply),
                            Ok(reply) if reply.status == "updated" || reply.status == "alive" => {}
                            _ => println!("{} gateway: {text}", "⚠️".yellow()),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return Ok(SessionEnd::Disconnected),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(Error::Api(format!("Live socket failed: {e}"))),
                }
            }
        }
    }
}

/// Waits for the gateway's next json reply, ignoring other frames.
async fn read_reply(socket: &mut WsStream) -> Result<GatewayReply> {
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

/// Prints one script log line, level colored by severity.
fn print_log_line(reply: &GatewayReply) {
    let level = reply.level.as_deref().unwrap_or("info");
    let label = match level {
        "error" => level.red(),
        "warn" => level.yellow(),
        "debug" => level.bright_black(),
        _ => level.cyan(),
    };

    println!("{:>5} {}", label, reply.message.as_deref().unwrap_or(""));
}

/// Wraps a payload in the `{event, data}` envelope the gateway routes on.
fn event_message(event: &str, data: serde_json::Value) -> Message {
    Message::text(serde_json::json!({ "event": event, "data": data }).to_string())
}

/// Reads the working tree into the live payload for `start` and `update`.
///
/// The config is re-read every time, so edits to `script.json` (new
/// includes, a different entry point) take effect on the next save.
fn live_payload(
    script_path: &Path,
    script_id: &str,
    session_id: Option<&str>,
) -> Result<serde_json::Value> {
    let config = ScriptConfig::from_path(script_path).map_err(Error::Script)?;
    let bundle = config.to_bundle().map_err(Error::Script)?;

    Ok(serde_json::json!({
        "scriptId": script_id,
        "sessionId": session_id,
        "revision": {
            "bundle": bundle,
            "scriptConfig": crate::client::types::ScriptConfigDto::from(config),
        }
    }))
}

/// Derives the live gateway socket url from the configured api url.
fn live_socket_url(api_url: &str) -> Result<String> {
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
