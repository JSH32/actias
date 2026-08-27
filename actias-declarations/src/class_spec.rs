//! A class body, normalized: the one reader of `publishes`, `receives`,
//! `migrations` and the routable-method set, shared by the extraction
//! pass (script-service and the cli, at publish and check) and the
//! worker's runtime registry. One reader is what keeps the stored
//! contract and the enforcing runtime in agreement by construction.

use std::collections::{HashMap, HashSet};

use mlua::Table;

/// Names only the platform may invoke on an object: the hooks (called
/// as `__`-prefixed internal methods) and their public spellings, which
/// handles refuse outright so a `__`-method arriving at dispatch is
/// provably platform-originated.
pub const RESERVED_METHODS: [&str; 6] = ["init", "alarm", "receive", "receives", "follow", "hooks"];

/// Whether a method name may travel through a handle.
pub fn callable_method(method: &str) -> bool {
    !method.starts_with('_') && !RESERVED_METHODS.contains(&method)
}

/// How a topic may be followed, as the runtime answers it: array
/// entries gate through `hooks.follow`, keyed entries carry a built-in
/// policy, anything else is not published at all. Policies: `"self"`
/// admits only the instance's own identity (personal streams);
/// `"public"` admits any identity (broadcast streams), which exists so
/// a public topic never needs a boilerplate yes-to-everyone gate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TopicPolicy {
    Absent,
    Hooked,
    SelfOnly,
    Public,
}

/// What a class IS, normalized once: routable method names, publish
/// policies, consumed streams, and where its schema comes from.
#[derive(Debug)]
pub struct ClassSpec {
    pub name: String,
    /// Names whose values are functions and which a handle may route;
    /// the worker's state fallthrough consults this before touching
    /// the table.
    pub methods: HashSet<String>,
    /// Topic to runtime policy, keyed entries overriding array ones.
    publishes: HashMap<String, TopicPolicy>,
    /// The publishes entries as the contract records them, in
    /// declaration order: `topic` for gated ones, `topic=policy` for
    /// keyed ones, unknown policies preserved verbatim.
    pub declared_publishes: Vec<String>,
    /// Consumed streams, "Source:topic", in declaration order.
    pub receives: Vec<String>,
    pub migrations: Option<String>,
}

impl ClassSpec {
    /// Parses a class body. Validation lives here so extraction and
    /// runtime refuse identically: the dead `hooks.receive` spelling,
    /// and malformed `receives` entries.
    pub fn parse(name: &str, body: &Table) -> mlua::Result<Self> {
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

        let mut receives = Vec::new();
        if let Ok(table) = body.get::<Table>("receives") {
            for pair in table.pairs::<mlua::Value, mlua::Value>() {
                let Ok((key, value)) = pair else { continue };
                let Some(stream) = key
                    .as_string()
                    .and_then(|s| s.to_str().ok().map(|s| s.to_string()))
                else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{name}': receives keys are \"Source:topic\" strings."
                    )));
                };
                if stream.split(':').filter(|part| !part.is_empty()).count() != 2 {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{name}': receives key '{stream}' is not \"Source:topic\"."
                    )));
                }
                if !value.is_function() {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{name}': receives['{stream}'] must be a handler function."
                    )));
                }
                receives.push(stream);
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

    pub fn topic_policy(&self, topic: &str) -> TopicPolicy {
        self.publishes
            .get(topic)
            .copied()
            .unwrap_or(TopicPolicy::Absent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

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
        assert!(spec.receives.contains(&"Auction:closed".to_string()));
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

    #[test]
    fn malformed_receives_refuse_identically_everywhere() {
        let lua = Lua::new();
        let shapeless: Table = lua
            .load(r#"{ receives = { badkey = function() end } }"#)
            .eval()
            .expect("evaluates");
        let refused = ClassSpec::parse("Room", &shapeless).expect_err("must refuse");
        assert!(refused.to_string().contains("is not \"Source:topic\""));

        let handlerless: Table = lua
            .load(r#"{ receives = { ["Chan:message"] = 5 } }"#)
            .eval()
            .expect("evaluates");
        let refused = ClassSpec::parse("Room", &handlerless).expect_err("must refuse");
        assert!(refused.to_string().contains("must be a handler function"));
    }
}
