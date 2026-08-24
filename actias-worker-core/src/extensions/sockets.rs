//! The sock value and the upgrade's worker-core half (S3 steps 3-4).
//!
//! Transport-agnostic on purpose: a sock speaks the object router and
//! mpsc channels, websocket framing lives in the worker binary, and no
//! ws type crosses below this line (the guest-neutral boundary).
//!
//! The one-key trick: `request.upgrade` is armed with a FUNCTION only
//! when the request actually carries a websocket handshake, so the
//! doc's two spellings share a mechanism: `if request.upgrade` is the
//! capability test, `request:upgrade(fn, identity)` is the act. The
//! call parks a [`PendingUpgrade`] in app data and returns a marker;
//! the HTTP layer takes the pending after the fetch handler returns,
//! completes the handshake, and drives [`run_connection`] in the SAME
//! vm, which is what makes closures legal.
//!
//! The sock is a plain Lua TABLE whose fields are async functions,
//! exactly the instance-handle pattern: Luau cannot yield across a
//! metamethod boundary, so userdata async methods are structurally
//! wrong here and a table of closures is the shape that works.

use std::sync::Arc;

use mlua::{Lua, LuaSerdeExt, Table};

use crate::connections::{InboxItem, InboxReceiver, OutboundFrame};
use crate::extensions::objects::{ObjectRouter, ObjectTarget};

/// Installs the `sockets` stdlib namespace at boot. `sockets.forward`
/// is the shipped program the doc prints, go-to-definition honest
/// because this IS its source.
///
/// Why `each` takes a CALLBACK and there is no generic-for iterator:
/// Luau (5.1 lineage) cannot resume a yield made inside a generic-for
/// iterator call ("attempt to yield across metamethod/C-call
/// boundary"), so a suspending `for item in sock:each()` is
/// structurally impossible. `sock:each(fn)` keeps the loop in the
/// platform (which is also where a per-wake budget naturally arms),
/// and `sock:recv()` is the awaitable primitive for programs that
/// want the loop themselves via `while`.
pub async fn install_prelude(lua: &Lua) -> mlua::Result<()> {
    const SOURCE: &str = r#"
        return {
            forward = function(target, topic, filter)
                return function(sock)
                    sock:follow(target, topic, filter)
                    sock:each(function(item)
                        if item.kind == "event" then
                            sock:send(item.event)
                        end
                    end)
                end
            end,
        }
    "#;
    let prelude: Table = lua
        .load(SOURCE)
        .set_name("=[actias.sockets]")
        .eval_async()
        .await?;
    let namespace = lua.create_table()?;
    namespace.set("forward", prelude.get::<mlua::Function>("forward")?)?;
    lua.globals().set("sockets", namespace)?;
    Ok(())
}

/// A requested upgrade, parked in app data until the HTTP layer picks
/// it up after the fetch handler returns.
pub struct PendingUpgrade {
    /// The connection program, held in the vm's registry.
    pub program: mlua::RegistryKey,
    /// The identity the connection speaks AS, minted after auth.
    pub class: String,
    pub name: String,
}

/// Arms `request.upgrade` on an upgradable request's table. Call this
/// ONLY when the underlying request can actually complete a websocket
/// handshake; absence of the key is the "cannot upgrade" signal.
pub fn arm_request(lua: &Lua, request: &Table) -> mlua::Result<()> {
    request.set(
        "upgrade",
        lua.create_function(
            |lua, (_this, program, identity): (Table, mlua::Function, Table)| {
                let class: String = identity.get("__class").map_err(|_| {
                    mlua::Error::RuntimeError(
                        "upgrade takes an identity handle: request:upgrade(fn, User(name))."
                            .to_owned(),
                    )
                })?;
                let name: String = identity.get("__name")?;
                if class.starts_with("__") {
                    return Err(mlua::Error::RuntimeError(
                        "a connection cannot speak as a platform class.".to_owned(),
                    ));
                }
                let pending = PendingUpgrade {
                    program: lua.create_registry_value(program)?,
                    class,
                    name,
                };
                if lua.set_app_data(pending).is_some() {
                    return Err(mlua::Error::RuntimeError(
                        "one upgrade per request.".to_owned(),
                    ));
                }
                // The marker the handler returns; the HTTP layer keys
                // on the parked PendingUpgrade, never on this shape.
                let marker = lua.create_table()?;
                marker.set("__actias_upgrade", true)?;
                Ok(marker)
            },
        )?,
    )
}

/// Everything one live connection holds, shared between the sock's
/// closures and [`run_connection`]'s cleanup. Built by the worker's
/// bridge after the handshake.
pub struct SockShared {
    pub connection_id: String,
    /// The identity this connection speaks as.
    pub class: String,
    pub name: String,
    inbox: Arc<tokio::sync::Mutex<InboxReceiver>>,
    outbound: tokio::sync::mpsc::Sender<OutboundFrame>,
    router: ObjectRouter,
    /// Edges this connection made, for polite cleanup on close; the
    /// pump's deliver-or-prune covers anything this list misses.
    follows: std::sync::Mutex<Vec<(String, String, String)>>,
}

impl SockShared {
    pub fn new(
        connection_id: String,
        class: String,
        name: String,
        inbox: InboxReceiver,
        outbound: tokio::sync::mpsc::Sender<OutboundFrame>,
        router: ObjectRouter,
    ) -> Arc<Self> {
        Arc::new(Self {
            connection_id,
            class,
            name,
            inbox: Arc::new(tokio::sync::Mutex::new(inbox)),
            outbound,
            router,
            follows: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The follower value every gate sees for this connection.
    fn follower_json(&self) -> serde_json::Value {
        serde_json::json!({
            "class": self.class,
            "name": self.name,
            "transport": "connection",
            "connection": self.connection_id,
        })
    }

    async fn edge_call(
        &self,
        method: &str,
        class: String,
        name: String,
        arguments: Vec<serde_json::Value>,
    ) -> mlua::Result<()> {
        (self.router)(ObjectTarget {
            class,
            name,
            method: method.to_owned(),
            arguments,
            chain: Vec::new(),
            caller: None,
        })
        .await
        .map(|_| ())
        .map_err(mlua::Error::RuntimeError)
    }
}

/// An instance handle's coordinates, or a friendly refusal.
fn target_of(handle: &Table) -> mlua::Result<(String, String)> {
    let class: String = handle.get("__class").map_err(|_| {
        mlua::Error::RuntimeError(
            "follow takes an instance handle: sock:follow(Room(id), topic).".to_owned(),
        )
    })?;
    let name: String = handle.get("__name")?;
    Ok((class, name))
}

/// Builds the sock table the connection program receives.
pub fn make_sock(lua: &Lua, shared: Arc<SockShared>) -> mlua::Result<Table> {
    let sock = lua.create_table()?;

    let this = shared.clone();
    sock.set(
        "follow",
        lua.create_async_function(
            move |lua,
                  (_sock, target, topic, filter): (
                mlua::Value,
                Table,
                String,
                Option<mlua::Value>,
            )| {
                let this = this.clone();
                async move {
                    let (class, name) = target_of(&target)?;
                    let filter = match filter {
                        Some(value) => lua.from_value::<serde_json::Value>(value)?,
                        None => serde_json::Value::Null,
                    };
                    this.edge_call(
                        "__follow",
                        class.clone(),
                        name.clone(),
                        vec![serde_json::json!(topic), filter, this.follower_json()],
                    )
                    .await?;
                    this.follows
                        .lock()
                        .expect("no panics hold the follow list")
                        .push((class, name, topic));
                    Ok(true)
                }
            },
        )?,
    )?;

    let this = shared.clone();
    sock.set(
        "unfollow",
        lua.create_async_function(
            move |_lua, (_sock, target, topic): (mlua::Value, Table, String)| {
                let this = this.clone();
                async move {
                    let (class, name) = target_of(&target)?;
                    this.edge_call(
                        "__unfollow",
                        class.clone(),
                        name.clone(),
                        vec![
                            serde_json::json!(topic),
                            serde_json::Value::Null,
                            this.follower_json(),
                        ],
                    )
                    .await?;
                    this.follows
                        .lock()
                        .expect("no panics hold the follow list")
                        .retain(|(c, n, t)| !(*c == class && *n == name && *t == topic));
                    Ok(true)
                }
            },
        )?,
    )?;

    let this = shared.clone();
    sock.set(
        "send",
        lua.create_async_function(move |lua, (_sock, value): (mlua::Value, mlua::Value)| {
            let this = this.clone();
            async move {
                let json = lua.from_value::<serde_json::Value>(value)?;
                this.outbound
                    .send(OutboundFrame::Json(json))
                    .await
                    .map_err(|_| {
                        mlua::Error::RuntimeError("the connection is closed.".to_owned())
                    })?;
                Ok(true)
            }
        })?,
    )?;

    let this = shared.clone();
    sock.set(
        "close",
        lua.create_function(move |_lua, _args: mlua::MultiValue| {
            let _ = this.outbound.try_send(OutboundFrame::Close);
            Ok(true)
        })?,
    )?;

    // `sock:recv()`: the awaitable primitive. One inbox, edge events
    // and client frames merged; nil after the wire closes, so
    // `while true do local item = sock:recv() ...` is the manual loop.
    let this = shared.clone();
    sock.set(
        "recv",
        lua.create_async_function(move |lua, _args: mlua::MultiValue| {
            let inbox = this.inbox.clone();
            async move {
                let next = inbox.lock().await.next().await;
                item_to_lua(&lua, next)
            }
        })?,
    )?;

    // `sock:each(fn)`: the platform owns the loop and calls the handler
    // per item, which sidesteps Luau's inability to resume a yield made
    // inside a generic-for iterator, keeps the not-pulled inbox as the
    // backpressure, and is where a per-wake budget will arm. The
    // handler returning `true` stops the loop (the duplex "break").
    let this = shared.clone();
    sock.set(
        "each",
        lua.create_async_function(
            move |lua, (_sock, handler): (mlua::Value, mlua::Function)| {
                let inbox = this.inbox.clone();
                async move {
                    loop {
                        let next = inbox.lock().await.next().await;
                        let Some(item) = item_to_lua(&lua, next)? else {
                            return Ok(());
                        };
                        let stop = handler
                            .call_async::<Option<bool>>(item)
                            .await?
                            .unwrap_or(false);
                        if stop {
                            return Ok(());
                        }
                    }
                }
            },
        )?,
    )?;

    Ok(sock)
}

/// One inbox item as the Lua tables the doc promises; None ends loops.
fn item_to_lua(lua: &Lua, next: Option<InboxItem>) -> mlua::Result<Option<Table>> {
    match next {
        None | Some(InboxItem::Closed) => Ok(None),
        Some(InboxItem::Frame(data)) => {
            let item = lua.create_table()?;
            item.set("kind", "frame")?;
            item.set("data", lua.to_value(&data)?)?;
            Ok(Some(item))
        }
        Some(InboxItem::Event {
            topic,
            from_class,
            from_name,
            data,
        }) => {
            let from = lua.create_table()?;
            from.set("id", format!("{from_class}/{from_name}"))?;
            from.set("class", from_class)?;
            from.set("name", from_name)?;
            let event = lua.create_table()?;
            event.set("topic", topic)?;
            event.set("from", from)?;
            event.set("data", lua.to_value(&data)?)?;
            let item = lua.create_table()?;
            item.set("kind", "event")?;
            item.set("event", event)?;
            Ok(Some(item))
        }
    }
}

/// Runs a connection program to completion in the surviving request vm,
/// then politely severs every edge the connection made (the pump's
/// deliver-or-prune covers anything this misses, so cleanup here is
/// courtesy, not correctness). The per-call budget is disarmed for the
/// connection's life; a bounded PER-WAKE budget is the recorded
/// refinement once the interrupt can be re-armed per inbox item.
pub async fn run_connection(
    runtime: &crate::runtime::ActiasRuntime,
    pending: PendingUpgrade,
    shared: Arc<SockShared>,
) -> Result<(), String> {
    runtime.end_call_budget();
    let program: mlua::Function = runtime
        .registry_value(&pending.program)
        .map_err(|e| e.to_string())?;
    let sock = make_sock(runtime, shared.clone()).map_err(|e| e.to_string())?;

    // The dispatch shape on purpose: an async C closure on the outside,
    // the Lua program call_async'd INSIDE its future, so nested async
    // calls suspend in the inner future instead of yielding through
    // the program's Lua frames (which Luau refuses).
    let driver = runtime
        .create_async_function(
            move |_lua, (program, sock): (mlua::Function, Table)| async move {
                program.call_async::<mlua::Value>(sock).await
            },
        )
        .map_err(|e| e.to_string())?;
    let outcome = driver
        .call_async::<mlua::Value>((program, sock))
        .await
        .map(|_| ())
        .map_err(|e| e.to_string());

    let made = shared
        .follows
        .lock()
        .expect("no panics hold the follow list")
        .clone();
    let follower = shared.follower_json();
    for (class, name, topic) in made {
        let _ = (shared.router)(ObjectTarget {
            class,
            name,
            method: "__unfollow".to_owned(),
            arguments: vec![
                serde_json::json!(topic),
                serde_json::Value::Null,
                follower.clone(),
            ],
            chain: Vec::new(),
            caller: None,
        })
        .await;
    }
    let _ = shared.outbound.send(OutboundFrame::Close).await;
    outcome
}
