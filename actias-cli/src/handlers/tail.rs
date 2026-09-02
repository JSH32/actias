//! Follow a published script's log lines in the terminal.

use std::time::Duration;

use colored::Colorize;
use futures::{SinkExt, StreamExt};
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

/// How one connected tail ended, deciding whether to reconnect.
enum TailEnd {
    Quit,
    Disconnected,
}

/// Runs `actias tail`: follows one script's published log channel.
///
/// # Errors
/// Returns the api's message, or the gateway's when the log socket
/// cannot be opened.
pub async fn handle(client: &Client, settings: &Settings, target: &str) -> Result<()> {
    let script_id = resolve_script_id(target)?;

    let script = client
        .get_script()
        .id(&script_id)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();

    let ws_url = gateway::live_socket_url(&settings.api_url)?;

    ui::step("Tailing", &script.public_identifier);

    let mut backoff = Duration::from_secs(1);
    loop {
        match tail_once(&ws_url, &settings.token, &script_id).await {
            Ok(TailEnd::Quit) => {
                ui::step("Stopped", "tail closed");
                return Ok(());
            }
            Ok(TailEnd::Disconnected) => {
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

/// Runs one connected tail until the user quits or the socket drops.
async fn tail_once(ws_url: &str, token: &str, script_id: &str) -> Result<TailEnd> {
    let mut socket = gateway::connect(ws_url, token).await?;

    socket
        .send(gateway::event_message(
            "tail",
            serde_json::json!({ "scriptId": script_id }),
        ))
        .await
        .map_err(|e| Error::Api(format!("Failed to start the tail: {e}")))?;

    let reply = gateway::read_reply(&mut socket).await?;
    if reply.status != "tailing" {
        return Err(Error::Api(format!(
            "The gateway answered '{}{}' instead of tailing",
            reply.status,
            reply.message.map(|m| format!(": {m}")).unwrap_or_default()
        )));
    }

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = socket.close(None).await;
                return Ok(TailEnd::Quit);
            }

            message = socket.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<GatewayReply>(&text) {
                            Ok(reply) if reply.status == "log" => gateway::print_log_line(&reply),
                            _ => ui::warn(format!("gateway: {text}")),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return Ok(TailEnd::Disconnected),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(Error::Api(format!("Live socket failed: {e}"))),
                }
            }
        }
    }
}

/// Reads the target as a project directory first, falling back to a raw
/// script id, so `actias tail .` works inside a project.
fn resolve_script_id(target: &str) -> Result<String> {
    if let Ok(path) = get_dir(target, false, false)
        && let Ok(config) = ScriptConfig::from_path(&path)
    {
        return config.id.ok_or_else(|| {
            Error::Script(format!(
                "this project has no script id yet; run {} once to create it",
                "actias publish".yellow()
            ))
        });
    }

    Ok(target.to_owned())
}
