//! A connection body, normalized: the one reader of the handler set,
//! shared by the extraction pass and the worker's runtime registry, the
//! same split as [`crate::class_spec`]. A connection is a visit, not a
//! place: nothing calls one, so its body declares handlers for the five
//! stimuli and nothing else.

use mlua::Table;

/// The handlers a connection body may declare, each optional.
pub const CONNECTION_HANDLERS: [&str; 4] = ["open", "frame", "event", "close"];

/// What a connection class IS, normalized once: its name and which
/// handlers the body declares.
#[derive(Debug)]
pub struct ConnectionSpec {
    pub name: String,
    /// The declared handler names, in [`CONNECTION_HANDLERS`] order.
    pub handlers: Vec<String>,
}

impl ConnectionSpec {
    /// Parses a connection body. Validation lives here so extraction
    /// and runtime refuse identically: unknown keys and non-function
    /// handlers die at declaration, which is what makes `check` a
    /// handler-shape verifier.
    pub fn parse(name: &str, body: &Table) -> mlua::Result<Self> {
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
        })
    }
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
}
