//! The `__cron` platform class: one instance per cron event, whose alarm
//! re-arms the next occurrence and fires the listener.
//!
//! The instance name is the event itself (`cron:<expr>`), so the class
//! needs no storage of its own beyond the alarm row. The re-arm happens
//! before the fire and listener failures are contained in
//! [`super::fire_listener`], so a failing handler can never kill the
//! schedule.

use crate::extensions::objects::{CRON_CLASS, cron_delay_ms, unix_now_ms};
use crate::runtime::ActiasRuntime;

/// Routes one `__cron` method call.
///
/// # Errors
/// Returns the user-safe text of whatever failed; an invalid expression
/// refuses `ensure` outright, the same way publish refuses it.
pub(crate) async fn dispatch(
    runtime: &ActiasRuntime,
    call: &super::Call,
) -> Result<serde_json::Value, String> {
    match call.method.as_str() {
        "ensure" => ensure(runtime, call),
        "alarm" => fire(runtime, call).await,
        other => Err(format!(
            "Object class '{CRON_CLASS}' has no method '{other}'."
        )),
    }
}

/// Arms the first occurrence; called once per revision per process, and
/// idempotent because setting an alarm replaces the previous one.
fn ensure(runtime: &ActiasRuntime, call: &super::Call) -> Result<serde_json::Value, String> {
    let delay_ms = cron_delay_ms(&call.name)?;
    super::set_alarm(runtime, CRON_CLASS, &call.name, delay_ms)?;
    Ok(serde_json::Value::Null)
}

/// Re-arms the next occurrence, then fires the listener; in that order,
/// so the schedule survives anything the handler does.
async fn fire(runtime: &ActiasRuntime, call: &super::Call) -> Result<serde_json::Value, String> {
    let delay_ms = cron_delay_ms(&call.name)?;
    super::set_alarm(runtime, CRON_CLASS, &call.name, delay_ms)?;

    let payload = serde_json::json!({
        "cron": call.name,
        "scheduled_at": unix_now_ms(),
    });
    super::fire_listener(runtime, &call.name, &payload).await;

    Ok(serde_json::Value::Null)
}
