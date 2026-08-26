//! The class registry: declared class bodies normalized into a typed
//! spec, once, at declaration. The lua table stays the home of the
//! functions themselves; the spec is the metadata every consumer reads
//! without another table walk.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mlua::{Lua, Table};

use super::dispatch::callable_method;

/// How a topic may be followed, as the runtime answers it: array
/// entries gate through `hooks.follow`, keyed entries carry a built-in
/// policy, anything else is not published at all. Policies: `"self"`
/// admits only the instance's own identity (personal streams);
/// `"public"` admits any identity (broadcast streams), which exists so
/// a public topic never needs a boilerplate yes-to-everyone gate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TopicPolicy {
    Absent,
    Hooked,
    SelfOnly,
    Public,
}

/// What a class IS, normalized once: routable method names, publish
/// policies, consumed streams, and where its schema comes from.
#[derive(Debug)]
pub(crate) struct ClassSpec {
    pub name: String,
    /// Names whose values are functions and which a handle may route;
    /// the state fallthrough consults this before touching the table.
    pub methods: HashSet<String>,
    /// Topic to runtime policy, keyed entries overriding array ones,
    /// exactly as the table reads answered before the spec existed.
    publishes: HashMap<String, TopicPolicy>,
    /// The publishes entries as the contract records them, in
    /// declaration order: `topic` for gated ones, `topic=policy` for
    /// keyed ones, unknown policies preserved verbatim.
    pub declared_publishes: Vec<String>,
    pub receives: HashSet<String>,
    pub migrations: Option<String>,
}

impl ClassSpec {
    /// Parses a class body. Validation lives here: the dead
    /// `hooks.receive` spelling is refused in parity with extraction.
    pub(super) fn parse(name: &str, body: &Table) -> mlua::Result<Self> {
        if let Ok(hooks) = body.get::<Table>("hooks")
            && hooks.contains_key("receive").unwrap_or(false)
        {
            return Err(mlua::Error::RuntimeError(format!(
                "'{name}' declares hooks.receive, which no longer exists: \
                 declare receives = {{ [\"Source:topic\"] = handler }} instead."
            )));
        }

        let mut methods = HashSet::new();
        for pair in body.pairs::<mlua::Value, mlua::Value>() {
            let Ok((key, value)) = pair else { continue };
            let Some(key) = key
                .as_string()
                .and_then(|s| s.to_str().ok().map(|s| s.to_string()))
            else {
                continue;
            };
            if matches!(value, mlua::Value::Function(_)) && callable_method(&key) {
                methods.insert(key);
            }
        }

        let mut publishes = HashMap::new();
        let mut declared_publishes = Vec::new();
        if let Ok(table) = body.get::<Table>("publishes") {
            let mut index = 1;
            while let Ok(entry) = table.get::<mlua::Value>(index) {
                if entry.is_nil() {
                    break;
                }
                if let Some(topic) = entry.as_string().and_then(|s| s.to_str().ok()) {
                    publishes.insert(topic.to_string(), TopicPolicy::Hooked);
                    declared_publishes.push(topic.to_string());
                }
                index += 1;
            }
            for pair in table.pairs::<mlua::Value, mlua::Value>() {
                let Ok((key, value)) = pair else { continue };
                let (Some(topic), Some(policy)) = (
                    key.as_string()
                        .and_then(|s| s.to_str().ok().map(|s| s.to_string())),
                    value
                        .as_string()
                        .and_then(|s| s.to_str().ok().map(|s| s.to_string())),
                ) else {
                    continue;
                };
                // Keyed entries override array ones; an unknown policy
                // is recorded in the contract but admits nobody.
                let answer = match policy.as_str() {
                    "self" => TopicPolicy::SelfOnly,
                    "public" => TopicPolicy::Public,
                    _ => TopicPolicy::Absent,
                };
                publishes.insert(topic.clone(), answer);
                declared_publishes.push(format!("{topic}={policy}"));
            }
        }

        let mut receives = HashSet::new();
        if let Ok(table) = body.get::<Table>("receives") {
            for pair in table.pairs::<mlua::Value, mlua::Value>() {
                let Ok((key, _)) = pair else { continue };
                if let Some(key) = key.as_string().and_then(|s| s.to_str().ok()) {
                    receives.insert(key.to_string());
                }
            }
        }

        Ok(ClassSpec {
            name: name.to_owned(),
            methods,
            publishes,
            declared_publishes,
            receives,
            migrations: body.get::<String>("migrations").ok(),
        })
    }

    pub(super) fn topic_policy(&self, topic: &str) -> TopicPolicy {
        self.publishes
            .get(topic)
            .copied()
            .unwrap_or(TopicPolicy::Absent)
    }
}

/// Registry key of the class-name-to-methods table in this vm; the
/// registry view below is the only door, so no sibling reads the slot
/// raw.
const CLASSES_KEY: &str = "object_classes";

/// The vm's specs by class name, app data beside the lua registry.
struct ClassSpecs(HashMap<String, Arc<ClassSpec>>);

/// The vm's class registry, one door for both halves: the lua side
/// (class body tables in the named registry, home of the functions)
/// and the rust side (the typed specs in app data). Declaration is a
/// single operation so the halves cannot drift.
pub(super) struct ClassRegistry<'lua> {
    lua: &'lua Lua,
}

impl<'lua> ClassRegistry<'lua> {
    /// The registry of the vm behind `lua`.
    pub(super) fn of(lua: &'lua Lua) -> Self {
        Self { lua }
    }

    /// Installs the empty class table at extension boot.
    pub(super) fn install(&self) -> mlua::Result<()> {
        let classes = self.lua.create_table()?;
        self.lua.set_named_registry_value(CLASSES_KEY, classes)
    }

    /// Declares a class: the body table becomes reachable under its
    /// name and its spec is recorded, together.
    pub(super) fn declare(&self, spec: ClassSpec, body: &Table) -> mlua::Result<()> {
        let classes: Table = self.lua.named_registry_value(CLASSES_KEY)?;
        classes.set(spec.name.as_str(), body)?;
        if self.lua.app_data_ref::<ClassSpecs>().is_none() {
            self.lua.set_app_data(ClassSpecs(HashMap::new()));
        }
        if let Some(mut specs) = self.lua.app_data_mut::<ClassSpecs>() {
            specs.0.insert(spec.name.clone(), Arc::new(spec));
        }
        Ok(())
    }

    /// The spec a declaration stored, or [`None`] for a class this vm
    /// never declared (platform classes never appear here).
    pub(super) fn spec(&self, class: &str) -> Option<Arc<ClassSpec>> {
        self.lua
            .app_data_ref::<ClassSpecs>()
            .and_then(|specs| specs.0.get(class).cloned())
    }

    /// One class's body table, the home of its functions; [`None`] for
    /// a class this vm never declared.
    pub(super) fn class_table(&self, class: &str) -> Option<Table> {
        self.lua
            .named_registry_value::<Table>(CLASSES_KEY)
            .ok()?
            .get(class)
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_body_normalizes_into_its_spec() {
        let lua = Lua::new();
        let body: Table = lua
            .load(
                r#"{
                    publishes = { "bids", events = "self", wide = "public", odd = "sideways" },
                    receives = { ["Auction:closed"] = function() end },
                    migrations = "migrations/Ledger",
                    hooks = { init = function() end },
                    watch = function(state, id) end,
                    snapshot = function(state) end,
                    notes = "not a method",
                }"#,
            )
            .eval()
            .expect("body evaluates");

        let spec = ClassSpec::parse("Ledger", &body).expect("parses");
        assert!(spec.methods.contains("watch") && spec.methods.contains("snapshot"));
        assert!(
            !spec.methods.contains("notes"),
            "non-functions are not methods"
        );
        assert!(
            !spec.methods.contains("hooks"),
            "reserved names are not methods"
        );
        assert!(matches!(spec.topic_policy("bids"), TopicPolicy::Hooked));
        assert!(matches!(spec.topic_policy("events"), TopicPolicy::SelfOnly));
        assert!(matches!(spec.topic_policy("wide"), TopicPolicy::Public));
        assert!(
            matches!(spec.topic_policy("odd"), TopicPolicy::Absent),
            "an unknown policy admits nobody"
        );
        assert!(matches!(spec.topic_policy("ghost"), TopicPolicy::Absent));
        assert!(spec.receives.contains("Auction:closed"));
        assert_eq!(spec.migrations.as_deref(), Some("migrations/Ledger"));
        assert!(spec.declared_publishes.contains(&"bids".to_string()));
        assert!(
            spec.declared_publishes
                .contains(&"odd=sideways".to_string())
        );
    }

    #[test]
    fn the_dead_receive_spelling_is_refused_at_parse() {
        let lua = Lua::new();
        let body: Table = lua
            .load(r#"{ hooks = { receive = function() end } }"#)
            .eval()
            .expect("body evaluates");
        let refused = ClassSpec::parse("Room", &body).expect_err("must refuse");
        assert!(refused.to_string().contains("hooks.receive"));
    }
}
