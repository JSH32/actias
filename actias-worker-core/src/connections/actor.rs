//! The connection's program rides an actor, not a captured vm.
//!
//! [`ConnectionTask`] owns everything one connection holds for its
//! life: the inbox receiver, the wire and edge state in
//! [`SockShared`], the declared program's spec, and the `conn.state`
//! blob. The vm is the ONE droppable part: built from the factory
//! when a handler needs it, dropped again after `hibernate_after` of
//! silence. Hibernated, the connection costs about a file
//! descriptor; an inbox item whose handler is declared rebuilds the
//! vm, and the blob and the follows never noticed. A class that
//! declared the forward policy instead of an `event` handler never
//! needs one for a delivery: the platform envelope goes straight to
//! the wire.
//!
//! Ordering per connection is the actor itself, the discipline object
//! mailboxes provide: handlers for one connection never run
//! concurrently. Each handler invocation is one platform call and
//! runs under the per-call budget, which is what the imperative loop
//! could never arm per item.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use mlua::LuaSerdeExt;

use actias_declarations::ConnectionSpec;

use crate::connections::{InboxItem, InboxReceiver, OutboundFrame};
use crate::extensions::sockets::{ConnectionRegistry, SockShared, conn_surface, event_value};
use crate::runtime::ActiasRuntime;

/// Largest `conn.state` as serialized json: the hibernation contract.
/// A session worth more bytes than this wants an object.
pub const STATE_CAP_BYTES: usize = 64 * 1024;

/// Builds a vm of the connection's revision on demand. The worker
/// supplies it; worker-core never dials a backend itself.
pub type VmFactory = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = Result<ActiasRuntime, String>> + Send>>
        + Send
        + Sync,
>;

/// What the node's connections cost right now, in plain atomics so no
/// metrics type crosses the guest-neutral boundary; the worker renders
/// them in its own exposition. `warm` counts tasks holding a vm,
/// `hibernated` counts tasks alive without one after a drop; a
/// connection that never ran a handler counts in neither.
#[derive(Default)]
pub struct ConnectionGauges {
    pub warm: AtomicI64,
    pub hibernated: AtomicI64,
    pub wakes: AtomicU64,
    pub wake_ms_total: AtomicU64,
}

/// One live connection's actor: pulls the inbox, drives the declared
/// handlers, keeps `conn.state` across invocations and across
/// hibernation.
pub struct ConnectionTask {
    inbox: InboxReceiver,
    shared: Arc<SockShared>,
    /// The declared connection class the upgrade named.
    spec: Arc<ConnectionSpec>,
    /// The json blob behind `conn.state`, seeded at upgrade and
    /// written back after every handler return.
    state: serde_json::Value,
    vm: Option<ActiasRuntime>,
    factory: VmFactory,
    /// Silence long enough to drop the vm; [`None`] never drops.
    hibernate_after: Option<Duration>,
    /// Whether the vm was dropped by the idle timeout (the next build
    /// is a wake, not a birth).
    hibernated: bool,
    gauges: Arc<ConnectionGauges>,
}

impl ConnectionTask {
    pub fn new(
        inbox: InboxReceiver,
        shared: Arc<SockShared>,
        spec: Arc<ConnectionSpec>,
        seed: serde_json::Value,
        factory: VmFactory,
        hibernate_after: Option<Duration>,
        gauges: Arc<ConnectionGauges>,
    ) -> Self {
        Self {
            inbox,
            shared,
            spec,
            state: seed,
            vm: None,
            factory,
            hibernate_after,
            hibernated: false,
            gauges,
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
        if self.vm.is_some() {
            self.gauges.warm.fetch_sub(1, Ordering::Relaxed);
        } else if self.hibernated {
            self.gauges.hibernated.fetch_sub(1, Ordering::Relaxed);
        }
        outcome.and(closed)
    }

    async fn serve(&mut self) -> Result<(), String> {
        self.invoke("open", None).await?;
        let period = self.spec.timer_every_ms.map(Duration::from_millis);
        // A timered connection stays warm: the next thing to run is
        // already scheduled, so dropping the vm would buy one
        // guaranteed wake per period, a rent nobody wants.
        let hibernate_after = if period.is_some() {
            None
        } else {
            self.hibernate_after
        };
        let mut next_tick = period.map(|every| tokio::time::Instant::now() + every);
        loop {
            // Ticks are a select arm, never inbox items: a full inbox
            // closes the connection, and a heartbeat must not be able
            // to do that.
            if let (Some(scheduled), Some(every)) = (next_tick, period) {
                let ticked = tokio::select! {
                    item = self.inbox.next() => Some(item),
                    _ = tokio::time::sleep_until(scheduled) => None,
                };
                match ticked {
                    Some(None) => return Ok(()),
                    Some(Some(item)) => {
                        if !self.handle(item).await? {
                            return Ok(());
                        }
                        continue;
                    }
                    None => {
                        // Lateness past the deadline is a missed
                        // count, and the next deadline realigns to
                        // the period grid instead of drifting.
                        let late = tokio::time::Instant::now().saturating_duration_since(scheduled);
                        let missed = (late.as_millis() / every.as_millis()) as u32;
                        self.invoke("timer", Some(ArgumentJson::Missed(missed)))
                            .await?;
                        next_tick = Some(scheduled + every * (missed + 1));
                        continue;
                    }
                }
            }

            let next = match hibernate_after {
                None => Ok(self.inbox.next().await),
                Some(after) => tokio::time::timeout(after, self.inbox.next()).await,
            };
            let item = match next {
                // Silence past the threshold: the vm goes, the fd,
                // the inbox, the blob and the follows stay. A vm that
                // was never built (or already went) counts nothing.
                Err(_elapsed) => {
                    if self.vm.take().is_some() {
                        self.hibernated = true;
                        self.gauges.warm.fetch_sub(1, Ordering::Relaxed);
                        self.gauges.hibernated.fetch_add(1, Ordering::Relaxed);
                    }
                    continue;
                }
                Ok(None) => return Ok(()),
                Ok(Some(item)) => item,
            };
            if !self.handle(item).await? {
                return Ok(());
            }
        }
    }

    /// One inbox item to its handler; false means the wire ended and
    /// serving is over.
    async fn handle(&mut self, item: InboxItem) -> Result<bool, String> {
        match item {
            InboxItem::Closed => return Ok(false),
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
                // The declared forward policy sends the platform
                // envelope straight to the wire: delivery costs a
                // serialize and a channel send, and a hibernated vm
                // stays down. A declared handler shapes the frame
                // itself; declaring neither drops the event without
                // building a vm to discover that.
                if self.spec.forwards {
                    let frame = crate::extensions::sockets::event_frame(
                        &topic,
                        &from_class,
                        &from_name,
                        &data,
                    );
                    let _ = self
                        .shared
                        .outbound_sender()
                        .send(OutboundFrame::Json(frame))
                        .await;
                } else {
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
        Ok(true)
    }

    /// Whether the declared program names this handler; the timer is
    /// its own table, so it is declared by its interval.
    fn declares(&self, handler: &str) -> bool {
        if handler == "timer" {
            self.spec.timer_every_ms.is_some()
        } else {
            self.spec
                .handlers
                .iter()
                .any(|declared| declared == handler)
        }
    }

    /// Runs one declared handler with a fresh conn surface over the
    /// current state blob. An undeclared handler ignores the item
    /// BEFORE any vm exists, so a Closed at a hibernated connection
    /// with no close handler never pays a wake.
    async fn invoke(
        &mut self,
        handler: &str,
        argument: Option<ArgumentJson>,
    ) -> Result<(), String> {
        if !self.declares(handler) {
            return Ok(());
        }
        if self.vm.is_none() {
            let started = std::time::Instant::now();
            self.vm = Some((self.factory)().await?);
            self.gauges.warm.fetch_add(1, Ordering::Relaxed);
            if self.hibernated {
                self.hibernated = false;
                self.gauges.hibernated.fetch_sub(1, Ordering::Relaxed);
                self.gauges.wakes.fetch_add(1, Ordering::Relaxed);
                self.gauges
                    .wake_ms_total
                    .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
            }
        }
        let vm = self.vm.as_ref().expect("ensured above");

        let Some(class_table) = ConnectionRegistry::of(vm).class_table(&self.spec.name) else {
            return Err(format!(
                "The revision does not declare connection '{}'.",
                self.spec.name
            ));
        };
        // The timer's function sits one table deeper than the named
        // handlers: `timer = { every, run }`.
        let function: mlua::Value = if handler == "timer" {
            class_table
                .get::<mlua::Table>("timer")
                .and_then(|timer| timer.get("run"))
                .map_err(|e| e.to_string())?
        } else {
            class_table.get(handler).map_err(|e| e.to_string())?
        };
        let mlua::Value::Function(function) = function else {
            return Ok(());
        };

        let conn = conn_surface(vm, self.shared.clone()).map_err(|e| e.to_string())?;
        conn.set(
            "state",
            vm.to_value(&self.state).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        conn.set("name", self.shared.name.clone())
            .map_err(|e| e.to_string())?;
        conn.set("class", self.shared.class.clone())
            .map_err(|e| e.to_string())?;

        let argument = match argument {
            None => mlua::Value::Nil,
            Some(ArgumentJson::Value(value)) => vm.to_value(&value).map_err(|e| e.to_string())?,
            Some(ArgumentJson::Missed(missed)) => mlua::Value::Integer(missed as i64),
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
        // Each invocation is one platform call, so the budget arms here
        // and closes below: a connection vm outlives its invocations,
        // and an open scope would charge the idle time between them to
        // whichever handler ran last.
        vm.start_timer();
        let outcome = driver
            .call_async::<mlua::Value>((function, conn.clone(), argument))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
        vm.end_call_budget();

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
    /// The timer's coalesced lateness: ticks that fired while a
    /// previous invocation was still running.
    Missed(u32),
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
