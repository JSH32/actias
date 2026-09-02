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
pub const RESERVED_METHODS: [&str; 8] = [
    "init",
    "alarm",
    "receive",
    "receives",
    "follow",
    "hooks",
    "admit",
    "directory",
];

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
    /// The declared lifespan, exactly as written ("30d"); what the
    /// contract records.
    pub expire_raw: Option<String>,
    /// The lifespan in seconds, what the claim stamps; absent = never.
    pub expire_secs: Option<u64>,
    /// Whether the class gates creation with an `admit` function.
    pub admits: bool,
    /// The directory field set the class declares, at version zero:
    /// publish mints the real version by comparing this set against
    /// the previous contract's. [`None`] for a class with no directory.
    ///
    /// Declared rather than discovered. A field set inferred from
    /// derived rows cannot distinguish "this field was added in a later
    /// publish" from "this row simply has no value for it", because a
    /// nil is absence, so a field's first-seen version would depend on
    /// which delta happened to fold first. A declaration answers
    /// exactly, at the moment the author changes it.
    pub directory: Option<actias_common::directory_spec::DirectorySpec>,
}

impl ClassSpec {
    /// Parses a class body. Validation lives here so extraction and
    /// runtime refuse identically: the dead `hooks.receive` spelling,
    /// and malformed `receives` entries.
    ///
    /// # Errors
    /// Returns [`mlua::Error::RuntimeError`] naming the offending
    /// declaration: the dead `hooks.receive` spelling, a malformed
    /// `receives` entry, a lifespan under a second, a gate that is not a
    /// function, or a directory field the codec refuses.
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

        // The lifecycle declarations: a lifespan must be a duration of
        // at least a second, a gate must be a function, and both refuse
        // here so check and the runtime refuse identically.
        let expire = body.get::<mlua::Value>("expire")?;
        let (expire_raw, expire_secs) = match &expire {
            mlua::Value::Nil => (None, None),
            mlua::Value::String(raw) => {
                let raw = raw.to_str()?.to_string();
                let ms = crate::duration::parse_duration_ms(&raw)
                    .map_err(|why| mlua::Error::RuntimeError(format!("'{name}' expire: {why}")))?;
                if ms < 1000 {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{name}' expire: '{raw}' is under a second; a lifespan                          that short is a bug, not a policy."
                    )));
                }
                (Some(raw), Some((ms / 1000) as u64))
            }
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{name}' expire must be a duration string like \"30d\"."
                )));
            }
        };
        let admits = match body.get::<mlua::Value>("admit")? {
            mlua::Value::Nil => false,
            mlua::Value::Function(_) => true,
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{name}' admit must be a function taking the instance name."
                )));
            }
        };
        // The directory row: a function of state alone, so it can be
        // re-derived from any restored copy without a lease. A class
        // with no storage has no state to derive from, which is a
        // declaration mistake worth naming at check rather than at the
        // first write.
        let directory = match body.get::<mlua::Value>("directory")? {
            mlua::Value::Nil => None,
            mlua::Value::Table(table) => {
                // init lives in `hooks` canonically; the flat spelling
                // is the deprecated long form. Both count as storage,
                // and checking only one wrongly refuses every class
                // written the canonical way.
                let has_init = body
                    .get::<Table>("hooks")
                    .ok()
                    .and_then(|hooks| hooks.get::<mlua::Value>("init").ok())
                    .is_some_and(|init| init.is_function())
                    || body
                        .get::<mlua::Value>("init")
                        .is_ok_and(|init| init.is_function());
                if body.get::<String>("migrations").is_err() && !has_init {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{name}' declares a directory but has no storage: \
                         a directory row derives from state, so the class \
                         needs migrations or an init that creates its schema."
                    )));
                }
                Some(Self::directory_spec(name, &table)?)
            }
            mlua::Value::Function(_) => {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{name}' directory is a function; it is a table naming \
                     the fields it exposes and where they come from:\n  \
                     directory = {{\n    \
                     from = function(state) return state.sql:query_one(\"SELECT * FROM lot\") end,\n    \
                     fields = {{ status = f.string, high_bid = f.integer }},\n  }}\n\
                     Declared fields are what let a query know a field is \
                     on every row rather than guessing from the rows it \
                     happens to see."
                )));
            }
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{name}' directory must be a table with `from` and `fields`."
                )));
            }
        };

        Ok(ClassSpec {
            name: name.to_owned(),
            methods,
            publishes,
            declared_publishes,
            receives,
            migrations: body.get::<String>("migrations").ok(),
            expire_raw,
            expire_secs,
            admits,
            directory,
        })
    }

    /// Reads one `directory = { from = ..., fields = ... }` table into
    /// the field set the contract records.
    ///
    /// The reading itself is [`crate::field_kit::plan`], shared with the
    /// runtime that fills the row, so a class publishing clean is a
    /// class the worker can derive.
    fn directory_spec(
        class: &str,
        table: &Table,
    ) -> mlua::Result<actias_common::directory_spec::DirectorySpec> {
        use actias_common::directory_spec::DirectorySpec;

        let declared: Vec<(String, String)> = crate::field_kit::plan(class, table)?
            .fields
            .into_iter()
            .map(|field| (field.name, field.kind))
            .collect();

        if declared.is_empty() {
            return Err(mlua::Error::RuntimeError(format!(
                "'{class}' directory declares no fields; a directory with \
                 nothing to query is a listing of names, which every class \
                 already has."
            )));
        }

        // Version zero: unminted. Publish compares this set against the
        // previous contract's and stamps the version, so a deploy that
        // changes no field renumbers nothing.
        Ok(DirectorySpec::new(0, declared))
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

    /// Parses a class body written as a Lua table literal, with the
    /// field kit installed the way every vm that runs a class body has
    /// it.
    fn parse(name: &str, source: &str) -> mlua::Result<ClassSpec> {
        let lua = Lua::new();
        crate::field_kit::install(&lua).expect("the kit installs");
        let body: Table = lua.load(source).eval().expect("body evaluates");
        ClassSpec::parse(name, &body)
    }

    #[test]
    fn a_directory_is_recorded_with_its_declared_fields() {
        let spec = parse(
            "Auction",
            r#"{
                migrations = "migrations/Auction",
                directory = {
                    from = function(state) return state.sql:query_one("SELECT * FROM lot") end,
                    fields = {
                        status = f.string,
                        high_bid = f.integer(function(lot) return tonumber(lot.high_bid) end),
                    },
                },
                bid = function(state, amount) end,
            }"#,
        )
        .expect("parses");
        let directory = spec.directory.as_ref().expect("declares a directory");
        // Sorted, so the contract entry is canonical and two publishes
        // of the same set compare equal as strings.
        assert_eq!(
            directory.fields,
            vec![
                ("high_bid".to_owned(), "integer".to_owned()),
                ("status".to_owned(), "string".to_owned()),
            ]
        );
        // Unminted: publish stamps the version after diffing against
        // the previous contract.
        assert_eq!(directory.dver, 0);
        assert!(
            !spec.methods.contains("directory"),
            "the directory hook is not callable through a handle"
        );
        assert!(spec.methods.contains("bid"));

        let plain = parse("Viewer", "{}").expect("parses");
        assert!(plain.directory.is_none());
    }

    #[test]
    fn a_directory_refuses_without_storage_or_when_it_is_malformed() {
        let directory = r#"directory = {
            from = function(state) return state end,
            fields = { status = f.string },
        }"#;

        let error = parse("Viewer", &format!("{{ {directory} }}"))
            .expect_err("a class with no storage has no state to derive from");
        assert!(error.to_string().contains("no storage"), "{error}");

        // init counts as storage: a class may create its own schema.
        parse(
            "Counter",
            &format!("{{ init = function(state) end, {directory} }}"),
        )
        .expect("the flat init spelling satisfies the storage requirement");

        // The canonical spelling puts init in `hooks`; missing it here
        // refused every class written the documented way.
        parse(
            "Auction",
            &format!("{{ hooks = {{ init = function(state) end }}, {directory} }}"),
        )
        .expect("hooks.init satisfies it too");

        // The bare function form names its replacement rather than
        // failing with a type error: it was the documented spelling
        // until fields became declared.
        let error = parse(
            "Auction",
            r#"{ migrations = "m", directory = function(state) return {} end }"#,
        )
        .expect_err("the function form is gone");
        assert!(error.to_string().contains("fields ="), "{error}");

        let error = parse(
            "Auction",
            r#"{ migrations = "m", directory = { from = function(s) return s end } }"#,
        )
        .expect_err("a from with no fields exposes nothing queryable");
        assert!(error.to_string().contains("fields"), "{error}");

        // A class declaring nothing but an empty set is a listing of
        // names, which every class already has.
        let error = parse(
            "Auction",
            r#"{ migrations = "m", directory = {
                from = function(s) return s end,
                fields = {},
            } }"#,
        )
        .expect_err("an empty set is not a directory");
        assert!(error.to_string().contains("no fields"), "{error}");

        // The grammar owns `any`, `all`, `none` and `name`; publish
        // refuses them here exactly as derivation would.
        let error = parse(
            "Auction",
            r#"{ migrations = "m", directory = {
                from = function(s) return s end,
                fields = { name = f.string },
            } }"#,
        )
        .expect_err("reserved");
        assert!(error.to_string().contains("reserved"), "{error}");
    }

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
    fn a_lifespan_and_a_gate_parse_and_refuse_identically() {
        let lua = Lua::new();
        let body: Table = lua
            .load(
                r#"{
                    expire = "30d",
                    admit = function(name) return #name > 3 end,
                    close = function(state) end,
                }"#,
            )
            .eval()
            .expect("body evaluates");
        let spec = ClassSpec::parse("Session", &body).expect("parses");
        assert_eq!(spec.expire_raw.as_deref(), Some("30d"));
        assert_eq!(spec.expire_secs, Some(30 * 86400));
        assert!(spec.admits);
        assert!(
            !spec.methods.contains("admit"),
            "the gate is platform-invoked, never routable"
        );
        assert!(spec.methods.contains("close"));

        for (bad, named) in [
            (r#"{ expire = "yesterday" }"#, "expire"),
            (r#"{ expire = 30 }"#, "duration string"),
            (r#"{ expire = "200ms" }"#, "under a second"),
            (r#"{ admit = "please" }"#, "admit must be a function"),
        ] {
            let body: Table = lua.load(bad).eval().expect("evaluates");
            let refused = ClassSpec::parse("Session", &body).expect_err("must refuse");
            assert!(
                refused.to_string().contains(named),
                "{bad} should name '{named}': {refused}"
            );
        }
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
