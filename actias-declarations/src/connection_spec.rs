//! A connection body, normalized: the one reader of the handler set,
//! shared by the extraction pass and the worker's runtime registry, the
//! same split as [`crate::class_spec`]. A connection is a visit, not a
//! place: nothing calls one, so its body declares handlers for the
//! wire's stimuli and nothing else.

use mlua::Table;

use crate::duration::parse_duration_ms;

/// The handlers a connection body may declare, each optional. `timer`
/// is separate: its value is a table, not a function, and its period
/// lives on the spec.
pub const CONNECTION_HANDLERS: [&str; 4] = ["open", "frame", "event", "close"];

/// What a connection class IS, normalized once: its name, which
/// handlers the body declares, and its heartbeat period when one is.
#[derive(Debug)]
pub struct ConnectionSpec {
    pub name: String,
    /// The declared handler names, in [`CONNECTION_HANDLERS`] order.
    pub handlers: Vec<String>,
    /// The declared `timer.every`, in milliseconds; [`None`] means no
    /// heartbeat. A timered connection stays warm.
    pub timer_every_ms: Option<u64>,
}

impl ConnectionSpec {
    /// Parses a connection body. Validation lives here so extraction
    /// and runtime refuse identically: unknown keys, non-function
    /// handlers and malformed timers die at declaration, which is what
    /// makes `check` a handler-shape verifier.
    pub fn parse(name: &str, body: &Table) -> mlua::Result<Self> {
        let mut timer_every_ms = None;
        for pair in body.pairs::<mlua::Value, mlua::Value>() {
            let (key, value) = pair?;
            let Some(key) = key
                .as_string()
                .and_then(|s| s.to_str().ok().map(|s| s.to_string()))
            else {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{name}': a connection body declares handlers by name."
                )));
            };
            if key == "timer" {
                timer_every_ms = Some(parse_timer(name, &value)?);
                continue;
            }
            if !CONNECTION_HANDLERS.contains(&key.as_str()) {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{name}' declares '{key}', which is not a connection \
                     handler. Nothing calls a connection, so a connection \
                     has handlers, not methods: open, frame, event, close."
                )));
            }
            if !matches!(value, mlua::Value::Function(_)) {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{name}': '{key}' must be a function."
                )));
            }
        }
        let handlers = CONNECTION_HANDLERS
            .iter()
            .filter(|handler| body.contains_key(**handler).unwrap_or(false))
            .map(|handler| (*handler).to_owned())
            .collect();
        Ok(Self {
            name: name.to_owned(),
            handlers,
            timer_every_ms,
        })
    }
}

/// The `timer` key's shape: `{ every = "30s", run = function }`.
fn parse_timer(name: &str, value: &mlua::Value) -> mlua::Result<u64> {
    let shape = || {
        mlua::Error::RuntimeError(format!(
            "'{name}': timer is declared as {{ every = \"30s\", run = function }}."
        ))
    };
    let mlua::Value::Table(timer) = value else {
        return Err(shape());
    };
    let every: String = timer.get::<Option<String>>("every")?.ok_or_else(shape)?;
    let run: mlua::Value = timer.get("run")?;
    if !matches!(run, mlua::Value::Function(_)) {
        return Err(shape());
    }
    let ms = parse_duration_ms(&every).map_err(|error| {
        mlua::Error::RuntimeError(format!("'{name}': timer.every: {error}"))
    })?;
    let ms = u64::try_from(ms).map_err(|_| shape())?;
    if ms < 1000 {
        return Err(mlua::Error::RuntimeError(format!(
            "'{name}': a timer under a second is a busy loop, not a heartbeat."
        )));
    }
    Ok(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> mlua::Result<ConnectionSpec> {
        let lua = mlua::Lua::new();
        let table: Table = lua.load(body).eval().expect("the body evaluates");
        ConnectionSpec::parse("Session", &table)
    }

    #[test]
    fn a_body_lists_its_handlers_in_canon_order() {
        let spec = parse("{ close = function() end, frame = function() end }")
            .expect("a handler-only body parses");
        assert_eq!(spec.handlers, vec!["frame", "close"]);
        assert_eq!(spec.timer_every_ms, None);
    }

    #[test]
    fn an_unknown_key_is_refused_naming_the_handlers() {
        let refused = parse("{ bid = function() end }").expect_err("must refuse");
        assert!(
            refused.to_string().contains("not a connection handler"),
            "{refused}"
        );
    }

    #[test]
    fn a_non_function_handler_is_refused() {
        let refused = parse("{ open = 5 }").expect_err("must refuse");
        assert!(
            refused.to_string().contains("must be a function"),
            "{refused}"
        );
    }

    #[test]
    fn a_timer_declares_its_period_and_bad_shapes_refuse() {
        let spec = parse(
            "{ timer = { every = \"30s\", run = function() end }, frame = function() end }",
        )
        .expect("a timered body parses");
        assert_eq!(spec.timer_every_ms, Some(30_000));

        for (body, tell) in [
            ("{ timer = function() end }", "declared as"),
            ("{ timer = { every = \"30s\" } }", "declared as"),
            ("{ timer = { every = \"oops\", run = function() end } }", "timer.every"),
            (
                "{ timer = { every = \"200ms\", run = function() end } }",
                "busy loop",
            ),
        ] {
            let refused = parse(body).expect_err("must refuse");
            assert!(refused.to_string().contains(tell), "{body}: {refused}");
        }
    }
}
