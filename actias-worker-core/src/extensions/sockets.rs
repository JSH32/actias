//! The connection declaration and the upgrade's worker-core half.
//!
//! Transport-agnostic on purpose: a conn speaks the object router and
//! mpsc channels, websocket framing lives in the worker binary, and no
//! ws type crosses below this line (the guest-neutral boundary).
//!
//! `connection "Class" { handlers }` declares the program at the top
//! level, the same curried shape as `object`; the registry below holds
//! the bodies and their specs in each vm that ran the entry point,
//! which is what lets a FRESH vm of the same revision reach the
//! program. That reachability is the whole design: the handlers run in
//! [`crate::connections::actor::ConnectionTask`], one invocation per
//! inbox item under the per-call budget, and the vm is rebuildable
//! rather than captured.
//!
//! The one-key trick: `request.upgrade` is armed with a FUNCTION only
//! when the request actually carries a websocket handshake, so the
//! doc's two spellings share a mechanism: `if request.upgrade` is the
//! capability test, `request:upgrade(Class, seed?, identity)` is the
//! act. The call parks a [`PendingUpgrade`] in app data and returns a
//! marker; the HTTP layer takes the pending after the fetch handler
//! returns, completes the handshake, and spawns the actor. Nothing
//! pins the request vm.
//!
//! There is deliberately NO stdlib connection program (a
//! `sockets.forward` existed briefly and was retracted in owner
//! review): a shipped forwarder would have let the platform's
//! internal envelope escape as a wire protocol nobody declared. The
//! app writes every handler that touches the wire, and the handlers
//! are named instead of inlined, which is what lets them survive the
//! vm.
//!
//! The one platform-defined frame is [`event_frame`], sent for a
//! class that declares `event = "forward"`. It is the same fast path
//! that retraction named its terms for, spelled as the policy a body
//! declares rather than a helper it calls: following something
//! without reshaping it costs no vm, so a hibernated connection stays
//! down.
//!
//! The conn is a plain Lua TABLE whose verb fields are async
//! functions, exactly the instance-handle pattern: Luau cannot yield
//! across a metamethod boundary, so userdata async methods are
//! structurally wrong here and a table of closures is the shape that
//! works.

use std::collections::HashMap;
use std::sync::Arc;

use mlua::{Lua, LuaSerdeExt, Table};

use actias_declarations::ConnectionSpec;

use crate::connections::OutboundFrame;
use crate::connections::actor::STATE_CAP_BYTES;
use crate::extensions::objects::{ObjectRouter, ObjectTarget};
use crate::runtime::ActiasRuntime;
use crate::runtime::extension::{ExtensionInfo, LuaExtension};

/// Registry key of the connection-class-name-to-body table in this vm;
/// the registry view below is the only door.
const CONNECTION_CLASSES_KEY: &str = "connection_classes";

/// The vm's connection specs by class name, app data beside the lua
/// registry, the same split as the object class registry.
struct ConnectionSpecs(HashMap<String, Arc<ConnectionSpec>>);

/// The vm's connection-class registry: declared bodies under their
/// names (the handlers' home) and the typed specs beside them.
pub struct ConnectionRegistry<'lua> {
    lua: &'lua Lua,
}

impl<'lua> ConnectionRegistry<'lua> {
    /// The registry of the vm behind `lua`.
    pub fn of(lua: &'lua Lua) -> Self {
        Self { lua }
    }

    /// Installs the empty class table at extension boot.
    fn install(&self) -> mlua::Result<()> {
        let classes = self.lua.create_table()?;
        self.lua
            .set_named_registry_value(CONNECTION_CLASSES_KEY, classes)
    }

    /// Declares a connection class: the body table becomes reachable
    /// under its name and its spec is recorded, together.
    fn declare(&self, spec: ConnectionSpec, body: &Table) -> mlua::Result<()> {
        let classes: Table = self.lua.named_registry_value(CONNECTION_CLASSES_KEY)?;
        classes.set(spec.name.as_str(), body)?;
        if self.lua.app_data_ref::<ConnectionSpecs>().is_none() {
            self.lua.set_app_data(ConnectionSpecs(HashMap::new()));
        }
        if let Some(mut specs) = self.lua.app_data_mut::<ConnectionSpecs>() {
            specs.0.insert(spec.name.clone(), Arc::new(spec));
        }
        Ok(())
    }

    /// The spec a declaration stored, or [`None`] for a class this vm
    /// never declared.
    pub fn spec(&self, class: &str) -> Option<Arc<ConnectionSpec>> {
        self.lua
            .app_data_ref::<ConnectionSpecs>()
            .and_then(|specs| specs.0.get(class).cloned())
    }

    /// One class's body table, the home of its handlers; [`None`] for a
    /// class this vm never declared.
    pub fn class_table(&self, class: &str) -> Option<Table> {
        self.lua
            .named_registry_value::<Table>(CONNECTION_CLASSES_KEY)
            .ok()?
            .get(class)
            .ok()
    }
}

pub struct ConnectionExtension;

impl LuaExtension for ConnectionExtension {
    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "connection",
            description: "Declared connection programs",
            default: true,
        }
    }

    fn create_extension(&self, lua: &Lua) -> mlua::Result<mlua::Value> {
        ConnectionRegistry::of(lua).install()?;

        // `connection "Class" { handlers }`, curried like `object`: the
        // name is checked and recorded, the table becomes the class
        // body in this vm, and the handle is what an upgrade call takes.
        let declaration = lua.create_function(|lua, class: String| {
            ActiasRuntime::assert_declaration_phase(lua, "connection")?;
            if class.starts_with("__") {
                return Err(mlua::Error::RuntimeError(format!(
                    "Class name '{class}' is reserved for the platform."
                )));
            }
            ActiasRuntime::record_connection_declaration(lua, &class);

            lua.create_function(move |lua, body: Table| {
                // One parse normalizes the body and carries the
                // validation, so the stored contract and this enforcing
                // runtime agree by construction.
                let spec = ConnectionSpec::parse(&class, &body)?;
                ConnectionRegistry::of(lua).declare(spec, &body)?;
                let handle = lua.create_table()?;
                handle.set("__connection", class.clone())?;
                Ok(handle)
            })
        })?;

        Ok(mlua::Value::Function(declaration))
    }
}

/// A requested upgrade, parked in app data until the HTTP layer picks
/// it up after the fetch handler returns. Plain data on purpose:
/// nothing here pins the request vm.
pub struct PendingUpgrade {
    /// The declared connection class that will run the wire. The whole
    /// spec travels (not just the name) so the actor can answer "is
    /// this handler even declared" without building a vm to ask.
    pub spec: Arc<ConnectionSpec>,
    /// The initial `conn.state`, json the moment it leaves the vm.
    pub seed: serde_json::Value,
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
            |lua,
             (_this, program, second, third): (
                Table,
                mlua::Value,
                mlua::Value,
                Option<Table>,
            )| {
                if matches!(program, mlua::Value::Function(_)) {
                    return Err(mlua::Error::RuntimeError(
                        "The connection program is declared, not closed over: \
                         declare `connection \"Name\" { ... }` at the top level \
                         and pass the class to upgrade."
                            .to_owned(),
                    ));
                }
                let spec_name: String = program
                    .as_table()
                    .and_then(|handle| handle.get("__connection").ok())
                    .ok_or_else(|| {
                        mlua::Error::RuntimeError(
                            "upgrade takes a declared connection class: \
                             request:upgrade(Session, seed?, User(name))."
                                .to_owned(),
                        )
                    })?;
                let Some(spec) = ConnectionRegistry::of(lua).spec(&spec_name) else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Connection '{spec_name}' is not declared in this script."
                    )));
                };

                // The seed is optional and sits between the class and
                // the identity: two args means no seed.
                let (seed, identity) = match third {
                    Some(identity) => (second, identity),
                    None => match second {
                        mlua::Value::Table(identity) => (mlua::Value::Nil, identity),
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "upgrade takes an identity handle: \
                                 request:upgrade(Session, seed?, User(name))."
                                    .to_owned(),
                            ));
                        }
                    },
                };
                let class: String = identity.get("__class").map_err(|_| {
                    mlua::Error::RuntimeError(
                        "upgrade takes an identity handle: \
                         request:upgrade(Session, seed?, User(name))."
                            .to_owned(),
                    )
                })?;
                let name: String = identity.get("__name")?;
                if class.starts_with("__") {
                    return Err(mlua::Error::RuntimeError(
                        "a connection cannot speak as a platform class.".to_owned(),
                    ));
                }

                let seed = match seed {
                    mlua::Value::Nil => serde_json::json!({}),
                    value => lua.from_value::<serde_json::Value>(value)?,
                };
                let size = seed.to_string().len();
                if size > STATE_CAP_BYTES {
                    return Err(mlua::Error::RuntimeError(format!(
                        "The state seed is {size} bytes; conn.state caps at \
                         {STATE_CAP_BYTES}. A session worth more belongs in an object."
                    )));
                }

                let pending = PendingUpgrade {
                    spec,
                    seed,
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

/// Everything one live connection holds outside its vm, shared between
/// the conn verbs and the actor's cleanup. Built by the worker's
/// bridge after the handshake.
pub struct SockShared {
    pub connection_id: String,
    /// The node hosting this socket; edges record it so a publisher
    /// homed elsewhere knows where to send.
    pub node: String,
    /// The identity this connection speaks as.
    pub class: String,
    pub name: String,
    outbound: tokio::sync::mpsc::Sender<OutboundFrame>,
    router: ObjectRouter,
    /// Edges this connection made, for polite cleanup on close; the
    /// pump's deliver-or-prune covers anything this list misses.
    follows: std::sync::Mutex<Vec<(String, String, String)>>,
}

impl SockShared {
    pub fn new(
        connection_id: String,
        node: String,
        class: String,
        name: String,
        outbound: tokio::sync::mpsc::Sender<OutboundFrame>,
        router: ObjectRouter,
    ) -> Arc<Self> {
        Arc::new(Self {
            connection_id,
            node,
            class,
            name,
            outbound,
            router,
            follows: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The follower value every gate sees for this connection.
    pub(crate) fn follower_value(&self) -> serde_json::Value {
        serde_json::json!({
            "class": self.class,
            "name": self.name,
            "transport": "connection",
            "connection": self.connection_id,
            "node": self.node,
        })
    }

    /// The edges this connection has made so far.
    pub(crate) fn follows_snapshot(&self) -> Vec<(String, String, String)> {
        self.follows
            .lock()
            .expect("no panics hold the follow list")
            .clone()
    }

    /// The channel the wire task drains.
    pub(crate) fn outbound_sender(&self) -> &tokio::sync::mpsc::Sender<OutboundFrame> {
        &self.outbound
    }

    /// One routed edge call with pre-built json arguments.
    pub(crate) async fn edge_call_raw(
        &self,
        method: &str,
        class: String,
        name: String,
        arguments: Vec<serde_json::Value>,
    ) -> Result<(), String> {
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
    }

    async fn edge_call(
        &self,
        method: &str,
        class: String,
        name: String,
        arguments: Vec<serde_json::Value>,
    ) -> mlua::Result<()> {
        self.edge_call_raw(method, class, name, arguments)
            .await
            .map_err(mlua::Error::RuntimeError)
    }
}

/// An instance handle's coordinates, or a friendly refusal.
fn target_of(handle: &Table) -> mlua::Result<(String, String)> {
    let class: String = handle.get("__class").map_err(|_| {
        mlua::Error::RuntimeError(
            "follow takes an instance handle: conn:follow(Room(id), topic).".to_owned(),
        )
    })?;
    let name: String = handle.get("__name")?;
    Ok((class, name))
}

/// Builds the verb half of the conn table a handler receives: follow,
/// unfollow, send and close. The actor lays `state`, `name` and
/// `class` over it per invocation.
pub fn conn_surface(lua: &Lua, shared: Arc<SockShared>) -> mlua::Result<Table> {
    let conn = lua.create_table()?;

    let this = shared.clone();
    conn.set(
        "follow",
        lua.create_async_function(
            move |lua,
                  (_conn, target, topic, filter): (
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
                        vec![serde_json::json!(topic), filter, this.follower_value()],
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
    conn.set(
        "unfollow",
        lua.create_async_function(
            move |_lua, (_conn, target, topic): (mlua::Value, Table, String)| {
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
                            this.follower_value(),
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
    conn.set(
        "send",
        lua.create_async_function(move |lua, (_conn, value): (mlua::Value, mlua::Value)| {
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
    conn.set(
        "close",
        lua.create_function(move |_lua, _args: mlua::MultiValue| {
            let _ = this.outbound.try_send(OutboundFrame::Close);
            Ok(true)
        })?,
    )?;

    Ok(conn)
}

/// The platform envelope a handler-less connection forwards to its
/// wire: the same event the `event` handler would have seen, wrapped
/// as `{ type = "event", topic, from, data }`. Built in serde so the
/// forward never needs a vm.
pub fn event_frame(
    topic: &str,
    from_class: &str,
    from_name: &str,
    data: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "type": "event",
        "topic": topic,
        "from": {
            "id": format!("{from_class}/{from_name}"),
            "class": from_class,
            "name": from_name,
        },
        "data": data,
    })
}

/// One delivered edge event as the table the `event` handler receives:
/// `{ topic, from = { id, class, name }, data }`.
pub fn event_value(
    lua: &Lua,
    topic: &str,
    from_class: &str,
    from_name: &str,
    data: &serde_json::Value,
) -> mlua::Result<Table> {
    let from = lua.create_table()?;
    from.set("id", format!("{from_class}/{from_name}"))?;
    from.set("class", from_class)?;
    from.set("name", from_name)?;
    let event = lua.create_table()?;
    event.set("topic", topic)?;
    event.set("from", from)?;
    event.set("data", lua.to_value(data)?)?;
    Ok(event)
}
