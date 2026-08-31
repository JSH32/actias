//! Live development: watch a project directory and mirror every save into a
//! live session the worker serves at `/_live/<identifier>/<session>/`.

use std::{path::Path, pin::Pin, time::Duration};

use colored::Colorize;
use futures::{SinkExt, StreamExt};
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::{
    client::Client,
    errors::{Error, Result, progenitor_error},
    gateway::{self, GatewayReply},
    script::ScriptConfig,
    settings::Settings,
    ui,
    util::get_dir,
};

/// How long a burst of file events settles before one update is sent.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Keeps an idle session alive; must stay well under the server's session
/// ttl, which is two minutes.
const HEARTBEAT: Duration = Duration::from_secs(30);

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
            "this project has no script id yet; run {} once to create it",
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

    let ws_url = gateway::live_socket_url(&settings.api_url)?;

    println!(
        "{} for {}",
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
                ui::step("Stopped", "session left to expire");
                return Ok(());
            }
            Ok(SessionEnd::Disconnected) => {
                ui::step("Reconnecting", "connection lost");
                backoff = Duration::from_secs(1);
            }
            Err(error) => {
                ui::warn(format!("{error} (retrying in {}s)", backoff.as_secs()));
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
    let mut socket = gateway::connect(ws_url, token).await?;

    // Starting sends the full working tree, so the session serves the state
    // on disk right now, not the last published revision.
    socket
        .send(gateway::event_message(
            "start",
            live_payload(script_path, script_id, None)?,
        ))
        .await
        .map_err(|e| Error::Api(format!("Failed to start the session: {e}")))?;

    let reply = gateway::read_reply(&mut socket).await?;
    let Some(session_id) = reply.session_id else {
        return Err(Error::Api(format!(
            "The gateway answered '{}{}' without a session id",
            reply.status,
            reply.message.map(|m| format!(": {m}")).unwrap_or_default()
        )));
    };

    println!(
        "{}",
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
                            .send(gateway::event_message("update", payload))
                            .await
                            .map_err(|e| Error::Api(format!("Failed to send the update: {e}")))?;
                        ui::done("Updated", chrono::Local::now().format("%H:%M:%S").to_string().bright_black());
                    }
                    // A save can be observed mid-write or with a broken
                    // config; report it and keep watching rather than dying.
                    Err(error) => ui::warn(error),
                }
            }

            _ = heartbeat.tick() => {
                socket
                    .send(gateway::event_message("ping", serde_json::Value::Null))
                    .await
                    .map_err(|e| Error::Api(format!("Failed to send the heartbeat: {e}")))?;
            }

            message = socket.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<GatewayReply>(&text) {
                            Ok(reply) if reply.status == "log" => gateway::print_log_line(&reply),
                            Ok(reply) if reply.status == "updated" || reply.status == "alive" => {}
                            _ => ui::warn(format!("gateway: {text}")),
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
