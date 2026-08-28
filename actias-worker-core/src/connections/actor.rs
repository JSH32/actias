//! The connection's program rides an actor, not a captured vm.
//!
//! [`ConnectionTask`] owns everything one connection holds for its
//! life: the inbox receiver, the wire and edge state in
//! [`SockShared`], the declared program's class name, and the
//! `conn.state` blob. The vm is the ONE droppable part: built from
//! the factory when a handler needs it, dropped never in this
//! increment (warm-only; hibernation is the recorded next step).
//!
//! Ordering per connection is the actor itself, the discipline object
//! mailboxes provide: handlers for one connection never run
//! concurrently. Each handler invocation is one platform call and
//! runs under the per-call budget, which is what the imperative loop
//! could never arm per item.

use std::sync::Arc;

use mlua::LuaSerdeExt;

use crate::connections::{InboxItem, InboxReceiver, OutboundFrame};
use crate::extensions::sockets::{ConnectionRegistry, SockShared, conn_surface, event_value};
use crate::runtime::ActiasRuntime;

/// Largest `conn.state` as serialized json: the hibernation contract.
/// A session worth more bytes than this wants an object.
pub const STATE_CAP_BYTES: usize = 64 * 1024;

/// Builds a vm of the connection's revision on demand. The worker
/// supplies it; worker-core never dials a backend itself.
pub type VmFactory = Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn Future<Output = Result<ActiasRuntime, String>> + Send>,
        > + Send
        + Sync,
>;

/// One live connection's actor: pulls the inbox, drives the declared
/// handlers, keeps `conn.state` across invocations.
pub struct ConnectionTask {
    inbox: InboxReceiver,
    shared: Arc<SockShared>,
    /// The declared connection class the upgrade named.
    spec: String,
    /// The json blob behind `conn.state`, seeded at upgrade and
    /// written back after every handler return.
    state: serde_json::Value,
    vm: Option<ActiasRuntime>,
    factory: VmFactory,
}

impl ConnectionTask {
    pub fn new(
        inbox: InboxReceiver,
        shared: Arc<SockShared>,
        spec: String,
        seed: serde_json::Value,
        factory: VmFactory,
    ) -> Self {
        Self {
            inbox,
            shared,
            spec,
            state: seed,
            vm: None,
            factory,
        }
    }

    /// Runs the connection to completion: `open` first, one handler
    /// per inbox item, `close` after the wire ends, then the polite
    /// unfollow walk. Handler errors end the connection; the pump's
    /// deliver-or-prune covers whatever the walk misses.
    pub async fn run(mut self) -> Result<(), String> {
        let outcome = self.serve().await;
        // The close handler still runs on an erroring connection: the
        // wire is ending either way, and cleanup is the hook's point.
        let closed = self.invoke("close", None).await;
        self.shared.sever_follows().await;
        let _ = self.shared.send_close().await;
        outcome.and(closed)
    }

    async fn serve(&mut self) -> Result<(), String> {
        self.invoke("open", None).await?;
        loop {
            let Some(item) = self.inbox.next().await else {
                return Ok(());
            };
            match item {
                InboxItem::Closed => return Ok(()),
                InboxItem::Frame(data) => {
                    let argument = ArgumentJson::Value(data);
                    self.invoke("frame", Some(argument)).await?;
                }
                InboxItem::Event {
                    topic,
                    from_class,
                    from_name,
                    data,
                } => {
                    let argument = ArgumentJson::Event {
                        topic,
                        from_class,
                        from_name,
                        data,
                    };
                    self.invoke("event", Some(argument)).await?;
                }
            }
        }
    }

    /// Runs one declared handler with a fresh conn surface over the
    /// current state blob; an undeclared handler ignores the item.
    async fn invoke(
        &mut self,
        handler: &str,
        argument: Option<ArgumentJson>,
    ) -> Result<(), String> {
        if self.vm.is_none() {
            self.vm = Some((self.factory)().await?);
        }
        let vm = self.vm.as_ref().expect("ensured above");

        let Some(class_table) = ConnectionRegistry::of(vm).class_table(&self.spec) else {
            return Err(format!(
                "The revision does not declare connection '{}'.",
                self.spec
            ));
        };
        let function: mlua::Value = class_table.get(handler).map_err(|e| e.to_string())?;
        let mlua::Value::Function(function) = function else {
            return Ok(());
        };

        let conn = conn_surface(vm, self.shared.clone()).map_err(|e| e.to_string())?;
        conn.set("state", vm.to_value(&self.state).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        conn.set("name", self.shared.name.clone())
            .map_err(|e| e.to_string())?;
        conn.set("class", self.shared.class.clone())
            .map_err(|e| e.to_string())?;

        let argument = match argument {
            None => mlua::Value::Nil,
            Some(ArgumentJson::Value(value)) => {
                vm.to_value(&value).map_err(|e| e.to_string())?
            }
            Some(ArgumentJson::Event {
                topic,
                from_class,
                from_name,
                data,
            }) => mlua::Value::Table(
                event_value(vm, &topic, &from_class, &from_name, &data)
                    .map_err(|e| e.to_string())?,
            ),
        };

        // The dispatch shape on purpose: an async C closure on the
        // outside, the handler call_async'd INSIDE its future, so
        // nested async calls suspend in the inner future instead of
        // yielding through the handler's Lua frames (which Luau
        // refuses).
        let driver = vm
            .create_async_function(
                |_lua, (function, conn, argument): (mlua::Function, mlua::Table, mlua::Value)| async move {
                    function
                        .call_async::<mlua::Value>((conn, argument))
                        .await
                },
            )
            .map_err(|e| e.to_string())?;
        // Each invocation is one platform call; the budget arms here.
        vm.start_timer();
        let outcome = driver
            .call_async::<mlua::Value>((function, conn.clone(), argument))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());

        // The blob is what survives the invocation, cap enforced:
        // refusing here (and closing, in run's error path) is the
        // contract that keeps a session small enough to hibernate.
        let state: mlua::Value = conn.get("state").map_err(|e| e.to_string())?;
        let state = vm
            .from_value::<serde_json::Value>(state)
            .map_err(|e| format!("conn.state must stay json-serializable: {e}"))?;
        let size = state.to_string().len();
        if size > STATE_CAP_BYTES {
            return Err(format!(
                "conn.state is {size} bytes; the cap is {STATE_CAP_BYTES}. \
                 A session worth more belongs in an object."
            ));
        }
        self.state = state;
        outcome
    }
}

/// A handler argument, carried as json until a vm exists to lift it.
enum ArgumentJson {
    Value(serde_json::Value),
    Event {
        topic: String,
        from_class: String,
        from_name: String,
        data: serde_json::Value,
    },
}

/// The actor's wire-ward half lives on [`SockShared`]; these are the
/// pieces `run` needs that the sock verbs do not expose.
impl SockShared {
    /// Politely severs every edge this connection made; the pump's
    /// deliver-or-prune covers anything this walk misses, so failures
    /// here cost nothing.
    pub async fn sever_follows(&self) {
        let made = self.follows_snapshot();
        let follower = self.follower_value();
        for (class, name, topic) in made {
            let _ = self
                .edge_call_raw(
                    "__unfollow",
                    class,
                    name,
                    vec![
                        serde_json::json!(topic),
                        serde_json::Value::Null,
                        follower.clone(),
                    ],
                )
                .await;
        }
    }

    /// Tells the wire task to close the socket.
    pub async fn send_close(&self) -> Result<(), String> {
        self.outbound_sender()
            .send(OutboundFrame::Close)
            .await
            .map_err(|_| "the wire is gone".to_owned())
    }
}
