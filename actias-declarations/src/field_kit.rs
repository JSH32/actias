//! The field kit: the markers a class writes its directory fields with,
//! and the one reader that turns a `directory` table into a plan.
//!
//! A field is named once, and its kind sits on the same line as the
//! value it describes:
//!
//! ```lua
//! directory = {
//!     from = function(state) return state.sql:query_one("SELECT * FROM lot") end,
//!     fields = {
//!         state    = f.string,
//!         high_bid = f.integer(function(lot) return tonumber(lot.high_bid) end),
//!     },
//! }
//! ```
//!
//! A marker used bare reads the key's own name from what `from`
//! returned; called, it takes an extractor that runs against the same
//! value. Both spellings are data: the declaration pass reads the table
//! to learn the field set, and the runtime reads it again to fill a row.
//! Nothing declares a field by running per-object code, so a field
//! cannot hide behind a branch that only some objects take.
//!
//! The kit is installed in the extraction vm and in the runtime vm from
//! here, so the markers an author writes at publish are the same values
//! the derivation walks.

use mlua::{Lua, Table};

use actias_common::directory_spec::{FIELD_KINDS, check_field_name};

/// The global the markers hang off.
pub const KIT_GLOBAL: &str = "f";

/// The marker key naming which kind a field holds; one of
/// [`FIELD_KINDS`], so the author's spelling and the contract's are the
/// same word.
const KIND_KEY: &str = "kind";

/// The marker key holding a called marker's extractor.
const EXTRACT_KEY: &str = "extract";

/// One field, as the declaration wrote it.
#[derive(Debug)]
pub struct PlannedField {
    /// The field name, which is the key it was written under.
    pub name: String,
    /// Its declared kind, one of [`FIELD_KINDS`].
    pub kind: String,
    /// The extractor a called marker carried; [`None`] for a bare
    /// marker, which reads [`PlannedField::name`] from the source.
    pub extract: Option<mlua::Function>,
}

/// A `directory` table, read: the shared derivation and the fields it
/// feeds, sorted by name so a row's field order never depends on Lua's
/// hash iteration order.
#[derive(Debug)]
pub struct DirectoryPlan {
    /// Runs once per derivation, against the object's read-only state;
    /// its result is what every field reads.
    pub from: mlua::Function,
    pub fields: Vec<PlannedField>,
}

/// Builds the kit, the value the [`KIT_GLOBAL`] global holds.
///
/// Each kind is one marker table carrying its own kind, callable to
/// take an extractor. The markers are readonly, so a class cannot
/// mutate the kit every other class in the vm reads.
///
/// # Errors
/// Returns the Lua error from building the tables.
pub fn kit(lua: &Lua) -> mlua::Result<Table> {
    let kit = lua.create_table()?;
    for kind in FIELD_KINDS {
        let marker = lua.create_table()?;
        marker.set(KIND_KEY, kind)?;

        let meta = lua.create_table()?;
        meta.set(
            "__call",
            lua.create_function(move |lua, (this, extract): (Table, mlua::Value)| {
                let mlua::Value::Function(extract) = extract else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "f.{kind} takes a function of the value `from` returned: \
                         f.{kind}(function(source) return source.field end)."
                    )));
                };
                let called = lua.create_table()?;
                called.set(KIND_KEY, this.get::<String>(KIND_KEY)?)?;
                called.set(EXTRACT_KEY, extract)?;
                called.set_readonly(true);
                Ok(called)
            })?,
        )?;
        meta.set_readonly(true);
        marker.set_metatable(Some(meta))?;
        marker.set_readonly(true);

        kit.set(kind, marker)?;
    }
    kit.set_readonly(true);
    Ok(kit)
}

/// Installs the kit as a global.
///
/// # Errors
/// Returns the Lua error from building or setting the kit.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let kit = kit(lua)?;
    lua.globals().set(KIT_GLOBAL, kit)
}

/// Reads one `directory = { from = ..., fields = ... }` table.
///
/// This is the only reader: the declaration pass calls it to record the
/// field set at publish, and the worker calls it to derive a row, so a
/// class that publishes clean is a class the runtime can fill.
///
/// # Errors
/// Names the class, and the field where there is one: a missing or
/// non-function `from`, a missing `fields` table, a key that is not a
/// name, a value that is not a marker, and every rule
/// [`check_field_name`] carries.
pub fn plan(class: &str, directory: &Table) -> mlua::Result<DirectoryPlan> {
    let from = match directory.get::<mlua::Value>("from")? {
        mlua::Value::Function(from) => from,
        mlua::Value::Nil if directory.contains_key("derive")? => {
            return Err(mlua::Error::RuntimeError(format!(
                "'{class}' directory has a `derive`; a directory now names \
                 each field's kind where it declares it:\n  \
                 directory = {{\n    \
                 from = function(state) return state.sql:query_one(\"SELECT * FROM lot\") end,\n    \
                 fields = {{ state = {KIT_GLOBAL}.string, \
                 high_bid = {KIT_GLOBAL}.integer(function(lot) return tonumber(lot.high_bid) end) }},\n  }}"
            )));
        }
        _ => {
            return Err(mlua::Error::RuntimeError(format!(
                "'{class}' directory needs a `from` function: it runs once \
                 against the object's state, and every field reads what it \
                 returns."
            )));
        }
    };

    let fields: Table = directory.get("fields").map_err(|_| {
        mlua::Error::RuntimeError(format!(
            "'{class}' directory needs a `fields` table naming each field \
             and its kind, as `state = {KIT_GLOBAL}.string`."
        ))
    })?;

    let mut planned = Vec::new();
    for pair in fields.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        let mlua::Value::String(name) = key else {
            return Err(mlua::Error::RuntimeError(format!(
                "'{class}' directory fields are named: write \
                 `fields = {{ state = {KIT_GLOBAL}.string }}`."
            )));
        };
        let name = name.to_str()?.to_string();
        check_field_name(&name).map_err(mlua::Error::RuntimeError)?;

        let marker = match &value {
            mlua::Value::Table(marker) => marker,
            _ => return Err(not_a_marker(class, &name, &value)),
        };
        let Ok(kind) = marker.get::<String>(KIND_KEY) else {
            return Err(not_a_marker(class, &name, &value));
        };
        if !FIELD_KINDS.contains(&kind.as_str()) {
            return Err(mlua::Error::RuntimeError(format!(
                "'{class}' directory field '{name}' holds kind '{kind}'; the kinds are {}.",
                FIELD_KINDS.join(", ")
            )));
        }
        let extract = match marker.get::<mlua::Value>(EXTRACT_KEY)? {
            mlua::Value::Nil => None,
            mlua::Value::Function(extract) => Some(extract),
            _ => return Err(not_a_marker(class, &name, &value)),
        };

        planned.push(PlannedField {
            name,
            kind,
            extract,
        });
    }

    planned.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(DirectoryPlan {
        from,
        fields: planned,
    })
}

/// The one message for a field whose value is not a kit marker, which
/// is where an author lands who wrote the kind as a bare string.
fn not_a_marker(class: &str, field: &str, value: &mlua::Value) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "'{class}' directory field '{field}' is a {}; a field takes a kind \
         marker, bare to read its own name (`{field} = {KIT_GLOBAL}.string`) \
         or called with an extractor \
         (`{field} = {KIT_GLOBAL}.integer(function(source) return source.n end)`). \
         The kinds are {}.",
        value.type_name(),
        FIELD_KINDS.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vm with the kit installed, and the `directory` table a class
    /// body would carry.
    fn planned(source: &str) -> mlua::Result<DirectoryPlan> {
        let lua = Lua::new();
        install(&lua).expect("installs");
        let directory: Table = lua.load(source).eval()?;
        plan("Auction", &directory)
    }

    #[test]
    fn bare_and_called_markers_both_declare_their_kind() {
        let plan = planned(
            r#"return {
                from = function(state) return state end,
                fields = {
                    state = f.string,
                    high_bid = f.integer(function(lot) return tonumber(lot.high_bid) end),
                    tags = f.array,
                },
            }"#,
        )
        .expect("plans");

        let declared: Vec<(&str, &str, bool)> = plan
            .fields
            .iter()
            .map(|field| {
                (
                    field.name.as_str(),
                    field.kind.as_str(),
                    field.extract.is_some(),
                )
            })
            .collect();
        // Sorted by name, and the kind is the contract's own spelling.
        assert_eq!(
            declared,
            vec![
                ("high_bid", "integer", true),
                ("state", "string", false),
                ("tags", "array", false),
            ]
        );
    }

    #[test]
    fn the_kind_a_field_declares_cannot_be_changed_through_the_kit() {
        // Every class in a vm reads the same markers, so a class that
        // could write through one would redeclare another class's
        // fields. Both the kit and its markers refuse.
        let error = planned(
            r#"f.string.kind = "integer"
               return { from = function(s) return s end, fields = { a = f.string } }"#,
        )
        .expect_err("a readonly marker refuses the write");
        assert!(error.to_string().contains("readonly"), "{error}");
    }

    #[test]
    fn a_string_kind_names_the_marker_it_should_have_been() {
        let error = planned(
            r#"return {
                from = function(state) return state end,
                fields = { state = "string" },
            }"#,
        )
        .expect_err("the old spelling refuses");
        let error = error.to_string();
        assert!(error.contains("'state'"), "{error}");
        assert!(error.contains("f.string"), "names the marker: {error}");
    }

    #[test]
    fn a_derive_names_the_form_that_replaced_it() {
        let error = planned(
            r#"return {
                fields = { state = f.string },
                derive = function(state) return { state = "open" } end,
            }"#,
        )
        .expect_err("a derive refuses");
        assert!(error.to_string().contains("from ="), "{error}");
    }

    #[test]
    fn a_missing_from_or_fields_table_refuses() {
        let error = planned(r#"return { fields = { state = f.string } }"#)
            .expect_err("fields alone are not a directory");
        assert!(error.to_string().contains("`from`"), "{error}");

        let error = planned(r#"return { from = function(state) return state end }"#)
            .expect_err("a from alone is not a directory");
        assert!(error.to_string().contains("`fields`"), "{error}");
    }

    #[test]
    fn an_extractor_that_is_not_a_function_refuses_where_it_is_written() {
        let lua = Lua::new();
        install(&lua).expect("installs");
        let error = lua
            .load("return f.integer(5)")
            .eval::<mlua::Value>()
            .expect_err("a non-function extractor refuses");
        assert!(error.to_string().contains("f.integer"), "{error}");
    }

    #[test]
    fn a_reserved_or_malformed_field_name_refuses() {
        let error = planned(
            r#"return {
                from = function(state) return state end,
                fields = { name = f.string },
            }"#,
        )
        .expect_err("`name` is the instance itself");
        assert!(error.to_string().contains("reserved"), "{error}");
    }
}
