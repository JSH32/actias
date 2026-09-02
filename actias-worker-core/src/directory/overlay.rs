//! The local overlay: a class's rows materialized into columns, on the
//! node that wants to query them.
//!
//! This is the only place columns exist. Everything durable carries
//! fields by name (the object's file, the deltas, the base, the object's
//! shipping manifest), and the overlay is rebuilt per generation from
//! those, which is what makes a newly discovered field a local rebuild
//! rather than schema change propagating across nodes and regions.
//!
//! Column names are generated from the manifest's field order by
//! [`Shape`], never from user text, so every identifier a predicate can
//! emit comes from a closed derivation.

use std::path::{Path, PathBuf};

use super::delta;
use super::manifest::Manifest;
use super::predicate::{Order, Where, order_clause, where_clause};
use super::shape::{FieldKind, NAME_COLUMN, Shape, Value};

/// One page of a listing.
#[derive(Debug, PartialEq)]
pub struct Page {
    pub entries: Vec<Entry>,
    /// Feeds the next call; absent on the last page.
    pub cursor: Option<String>,
}

/// One object's row as a caller sees it.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub object_id: String,
    pub fields: Vec<(String, Value)>,
}

/// One row with the version it was indexed at, for the verified read:
/// matching it against the object's own manifest decides whether any
/// recomputation is needed at all.
#[derive(Debug, PartialEq)]
pub struct Candidate {
    pub entry: Entry,
    pub version: super::version::RowVersion,
}

/// One page of candidates; [`Page`] with the versions kept.
#[derive(Debug, PartialEq)]
pub struct CandidatePage {
    pub candidates: Vec<Candidate>,
    pub cursor: Option<String>,
}

/// What a caller asks for.
#[derive(Debug, Default)]
pub struct Query {
    pub where_: Where,
    pub order: Vec<Order>,
    pub limit: i64,
    pub cursor: Option<String>,
}

/// A class's rows, materialized and indexed for querying.
pub struct Overlay {
    path: PathBuf,
    shape: Shape,
    /// The base generation this was built from; a newer manifest means
    /// this overlay is stale and the caller rebuilds.
    pub generation: u64,
}

/// Binds one encoded pair as the sqlite type its kind names, so
/// comparisons and ordering behave the way the field itself does.
fn typed(kind: &str, value: &str) -> Box<dyn rusqlite::ToSql> {
    match kind {
        "integer" => match value.parse::<i64>() {
            Ok(number) => Box::new(number),
            Err(_) => Box::new(value.to_owned()),
        },
        "number" => match value.parse::<f64>() {
            Ok(number) => Box::new(number),
            Err(_) => Box::new(value.to_owned()),
        },
        "boolean" => Box::new(value == "true"),
        // Text and arrays alike: an array's json text is what
        // json_each reads.
        _ => Box::new(value.to_owned()),
    }
}

/// Maps a manifest's recorded kind onto the predicate family it admits.
fn family(kind: &str) -> FieldKind {
    match kind {
        "array" => FieldKind::Many,
        _ => FieldKind::Single,
    }
}

/// One delta's rows upserted into an overlay under last-writer-wins on
/// `(epoch, tombstone, rev, dver)`.
fn apply_delta(
    connection: &rusqlite::Connection,
    shape: &Shape,
    bytes: &[u8],
    scratch: &Path,
) -> Result<(), String> {
    let (rows, _) = delta::read(bytes, scratch)?;
    for row in rows {
        let mut names = vec!["object_id", "name", "epoch", "rev", "dver", "tombstone"];
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(row.object_id.clone()),
            Box::new(row.name.clone()),
            Box::new(row.epoch as i64),
            Box::new(row.snapshot.rev),
            Box::new(row.snapshot.dver),
            Box::new(row.tombstone as i64),
        ];
        for pair in &row.snapshot.fields {
            let Some(column) = shape.slot(&pair.field) else {
                continue;
            };
            names.push(Box::leak(column.into_boxed_str()));
            values.push(typed(&pair.kind, &pair.value));
        }
        let placeholders = vec!["?"; names.len()].join(", ");
        let updates: Vec<String> = names
            .iter()
            .skip(1)
            .map(|name| format!("{name} = excluded.{name}"))
            .collect();
        let sql = format!(
            "INSERT INTO rows ({}) VALUES ({placeholders})
             ON CONFLICT (object_id) DO UPDATE SET {}
             WHERE (excluded.epoch, excluded.tombstone, excluded.rev, excluded.dver)
                 > (rows.epoch, rows.tombstone, rows.rev, rows.dver)",
            names.join(", "),
            updates.join(", ")
        );
        let bound: Vec<&dyn rusqlite::ToSql> = values.iter().map(|value| value.as_ref()).collect();
        connection
            .execute(&sql, bound.as_slice())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Overlay {
    /// Materializes the class at `path`, replacing whatever was there.
    ///
    /// Delta rows are applied over the base as upserts rather than
    /// merged first: application is idempotent under last-writer-wins,
    /// so a delta seen twice changes nothing, and the overlay can
    /// absorb deltas the compactor has not folded yet.
    ///
    /// # Errors
    /// Returns SQLite's or the filesystem's message.
    pub fn build<D: AsRef<[u8]>>(
        base: Option<&[u8]>,
        deltas: &[D],
        manifest: &Manifest,
        path: &Path,
        scratch: &Path,
    ) -> Result<Overlay, String> {
        let declared: Vec<(String, FieldKind)> = manifest
            .fields
            .iter()
            .map(|field| (field.name.clone(), family(&field.kind)))
            .collect();
        let shape = Shape::declare(&declared)?;
        // Built beside its final name and renamed into place at the end,
        // so a query that opens the path meanwhile sees the previous
        // overlay complete rather than this one half-written. A cache,
        // rebuilt from immutable files: the durability pragmas would
        // only slow the build, and one transaction around every applied
        // row is what keeps a large class from paying one fsync per row.
        let building = path.with_extension("building");
        let _ = std::fs::remove_file(&building);
        let connection = rusqlite::Connection::open(&building).map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = OFF;
                 PRAGMA synchronous = OFF;
                 BEGIN;",
            )
            .map_err(|e| e.to_string())?;

        let columns: Vec<String> = manifest
            .fields
            .iter()
            .filter_map(|field| shape.slot(&field.name))
            .collect();
        let declarations: String = columns
            .iter()
            .map(|column| format!(", {column}"))
            .collect::<Vec<_>>()
            .join("");
        connection
            .execute_batch(&format!(
                "CREATE TABLE rows (
                     object_id TEXT PRIMARY KEY,
                     name      TEXT NOT NULL,
                     epoch     INTEGER NOT NULL,
                     rev       INTEGER NOT NULL,
                     dver      INTEGER NOT NULL,
                     tombstone INTEGER NOT NULL
                     {declarations}
                 );
                 CREATE INDEX rows_name ON rows (name);"
            ))
            .map_err(|e| e.to_string())?;
        // One index per field: rows are small and a class holds few
        // fields, so an ordered top-k is an index walk rather than a
        // sort over everything that matched.
        for column in &columns {
            connection
                .execute_batch(&format!("CREATE INDEX rows_{column} ON rows ({column});"))
                .map_err(|e| e.to_string())?;
        }

        if let Some(base) = base {
            apply_delta(&connection, &shape, base, scratch)?;
        }
        for bytes in deltas {
            apply_delta(&connection, &shape, bytes.as_ref(), scratch)?;
        }
        // WAL from here on, persistently: later deltas are applied in
        // place while queries read, and WAL is what lets a reader never
        // wait on (or see half of) a writer.
        connection
            .execute_batch("COMMIT; PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;
        drop(connection);
        std::fs::rename(&building, path).map_err(|e| e.to_string())?;
        Ok(Overlay {
            path: path.to_path_buf(),
            shape,
            generation: manifest.generation,
        })
    }

    /// A second handle on the same file, for a cache entry that
    /// records more deltas applied to it.
    pub fn reopen(other: &Overlay) -> Overlay {
        Overlay {
            path: other.path.clone(),
            shape: other.shape.clone(),
            generation: other.generation,
        }
    }

    /// Applies deltas that arrived after the build, in place. Upserts
    /// are idempotent under last-writer-wins, so applying a delta a
    /// second time changes nothing, and a reader mid-query sees the
    /// overlay before or after each delta, never between rows of one.
    /// This is what keeps a hot class from rebuilding its whole
    /// overlay on every flush: O(delta) per delta instead of O(rows).
    ///
    /// # Errors
    /// Returns SQLite's or the filesystem's message.
    pub fn apply<D: AsRef<[u8]>>(&self, deltas: &[D], scratch: &Path) -> Result<(), String> {
        let connection = rusqlite::Connection::open(&self.path).map_err(|e| e.to_string())?;
        connection
            .execute_batch("PRAGMA synchronous = OFF; BEGIN;")
            .map_err(|e| e.to_string())?;
        for bytes in deltas {
            apply_delta(&connection, &self.shape, bytes.as_ref(), scratch)?;
        }
        connection
            .execute_batch("COMMIT;")
            .map_err(|e| e.to_string())
    }

    /// Answers one listing.
    ///
    /// # Errors
    /// Refuses an unknown field, a field still building, and the
    /// grammar's own refusals; otherwise returns SQLite's message.
    pub fn list(&self, query: &Query, manifest: &Manifest) -> Result<Page, String> {
        let page = self.candidates(query, manifest)?;
        Ok(Page {
            entries: page
                .candidates
                .into_iter()
                .map(|candidate| candidate.entry)
                .collect(),
            cursor: page.cursor,
        })
    }

    /// Answers one listing with each row's version, for the verified
    /// read: a candidate whose `(epoch, rev, dver)` matches the
    /// object's own manifest needs no recomputation at all, so the
    /// version travels with the row instead of being looked up again.
    ///
    /// [`Self::list`] is this with the versions dropped; one query
    /// path, so the two can never disagree about what a page is.
    ///
    /// # Errors
    /// Refuses an unknown field, a field still building, and the
    /// grammar's own refusals; otherwise returns SQLite's message.
    pub fn candidates(&self, query: &Query, manifest: &Manifest) -> Result<CandidatePage, String> {
        let building: std::collections::HashSet<String> = manifest
            .building()
            .into_iter()
            .map(|field| field.name.clone())
            .collect();

        let (predicate, params) = where_clause(&query.where_, &self.shape, &building)?;
        // Name is appended as the final key so the order is total: two
        // rows sharing every sort value still have one answer, which is
        // what makes a cursor able to resume exactly.
        let mut order = query.order.clone();
        if !order.iter().any(|entry| entry.field == NAME_COLUMN) {
            order.push(Order {
                field: NAME_COLUMN.to_owned(),
                descending: false,
            });
        }
        let ordering = order_clause(&order, &self.shape, &building)?;

        let mut sql = format!(
            "SELECT object_id, name, epoch, rev, dver{} FROM rows WHERE tombstone = 0 AND {predicate}",
            self.selected_columns()
        );
        let mut bound: Vec<Value> = params;
        if let Some(cursor) = &query.cursor {
            let (clause, keys) = self.after(cursor, &order, &building)?;
            sql.push_str(" AND ");
            sql.push_str(&clause);
            bound.extend(keys);
        }
        sql.push(' ');
        sql.push_str(&ordering);
        sql.push_str(&format!(" LIMIT {}", query.limit.max(1)));

        let connection = rusqlite::Connection::open(&self.path).map_err(|e| e.to_string())?;
        let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
        // Names paired with the kind the manifest recorded, so a value
        // comes back as the kind it went in as rather than as text.
        let names: Vec<(String, String)> = self
            .shape
            .fields()
            .iter()
            .map(|(name, _)| {
                let kind = manifest
                    .field(name)
                    .map(|field| field.kind.clone())
                    .unwrap_or_else(|| "string".to_owned());
                (name.clone(), kind)
            })
            .collect();
        let candidates = statement
            .query_map(rusqlite::params_from_iter(bound.iter()), |row| {
                let mut fields = Vec::new();
                for (index, (name, kind)) in names.iter().enumerate() {
                    let cell = row.get_ref(index + 5)?;
                    if let Some(value) = Self::read_cell(cell, kind) {
                        fields.push((name.clone(), value));
                    }
                }
                Ok(Candidate {
                    entry: Entry {
                        object_id: row.get(0)?,
                        name: row.get(1)?,
                        fields,
                    },
                    version: super::version::RowVersion {
                        epoch: row.get::<_, i64>(2)?.max(0) as u64,
                        rev: row.get::<_, i64>(3)?.max(0) as u64,
                        dver: row.get::<_, i64>(4)?.max(0) as u64,
                    },
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        // A full page implies there may be more; a short one is the
        // last. Never a count, which would cost a second scan.
        let cursor = (candidates.len() as i64 >= query.limit.max(1))
            .then(|| {
                candidates
                    .last()
                    .map(|candidate| candidate.entry.name.clone())
            })
            .flatten();

        Ok(CandidatePage { candidates, cursor })
    }

    /// Every live row's identity and the epoch it is held at, for
    /// reconciling the index against the objects that still exist.
    ///
    /// No predicate, no paging and no field columns: a reconciliation
    /// pass wants the whole set at once, and this is three columns per
    /// row rather than the full width. Tombstoned rows are skipped
    /// because they are already retired; only what the index still
    /// answers with can be wrong about existing.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn identities(&self) -> Result<Vec<super::repair::Indexed>, String> {
        let connection = rusqlite::Connection::open(&self.path).map_err(|e| e.to_string())?;
        let mut statement = connection
            .prepare("SELECT object_id, name, epoch FROM rows WHERE tombstone = 0")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(super::repair::Indexed {
                    object_id: row.get(0)?,
                    name: row.get(1)?,
                    epoch: row.get::<_, i64>(2)? as u64,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// Rows derived under a declaration older than `dver`, oldest
    /// first, capped at `limit`.
    ///
    /// The backfill's worklist. Oldest first so a pass that cannot
    /// finish still makes the floor move: `min_dver` is the lowest dver
    /// across live rows, so clearing the laggards is what lifts it, and
    /// clearing them in any other order would leave the floor where it
    /// was. Tombstones are skipped because a destroyed object has
    /// nothing to re-derive, and so are placeholders (rev 0, an
    /// identity repair found with nothing ever derived): there is no
    /// settled state to restore, and the floor ignores them too.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn behind(&self, dver: u64, limit: usize) -> Result<Vec<super::repair::Indexed>, String> {
        let connection = rusqlite::Connection::open(&self.path).map_err(|e| e.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT object_id, name, epoch FROM rows
                 WHERE tombstone = 0 AND rev > 0 AND dver < ?
                 ORDER BY dver, name LIMIT ?",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(rusqlite::params![dver as i64, limit as i64], |row| {
                Ok(super::repair::Indexed {
                    object_id: row.get(0)?,
                    name: row.get(1)?,
                    epoch: row.get::<_, i64>(2)?.max(0) as u64,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// One stored cell as the field value it represents. A null cell
    /// is an absent field, which is why this answers [`None`] rather
    /// than inventing a value.
    fn read_cell(cell: rusqlite::types::ValueRef<'_>, kind: &str) -> Option<Value> {
        use rusqlite::types::ValueRef;
        match cell {
            ValueRef::Null => None,
            ValueRef::Integer(number) if kind == "boolean" => Some(Value::Bool(number != 0)),
            ValueRef::Integer(number) => Some(Value::Integer(number)),
            ValueRef::Real(number) => Some(Value::Number(number)),
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                if kind == "array" {
                    // An array cell holds its json text; it comes back
                    // as the members, the way a delta row decodes.
                    super::row::decode(kind, &text).ok()
                } else {
                    Some(Value::Text(text))
                }
            }
        }
    }

    fn selected_columns(&self) -> String {
        self.shape
            .fields()
            .iter()
            .filter_map(|(name, _)| self.shape.slot(name))
            .map(|column| format!(", {column}"))
            .collect::<Vec<_>>()
            .join("")
    }

    /// The keyset clause resuming after `cursor`.
    ///
    /// Keyed on the instance name alone, which the order always ends
    /// with: a cursor over the full sort tuple would have to survive a
    /// row's sort values changing between pages, and a name cannot
    /// change. The cost is that a page boundary can repeat or skip a
    /// row whose sort value moved under it, which is the same
    /// staleness the whole index already has.
    fn after(
        &self,
        cursor: &str,
        order: &[Order],
        building: &std::collections::HashSet<String>,
    ) -> Result<(String, Vec<Value>), String> {
        let descending = order
            .last()
            .filter(|entry| entry.field == NAME_COLUMN)
            .is_some_and(|entry| entry.descending);
        // Resolved rather than hardcoded, so a building or unknown name
        // field refuses the same way any other would.
        let column = super::predicate::resolve_for_cursor(NAME_COLUMN, &self.shape, building)?;
        let comparison = if descending { "<" } else { ">" };
        Ok((
            format!("{column} {comparison} ?"),
            vec![Value::Text(cursor.to_owned())],
        ))
    }
}

#[cfg(test)]
mod tests {
    /// A build with no deltas; the parameter is generic over anything
    /// that lends bytes, so the empty case names its type.
    const NO_DELTAS: &[Vec<u8>] = &[];

    use super::super::delta::DeltaRow;
    use super::super::manifest::Field;
    use super::super::predicate::{Compare, Condition};
    use super::super::row::{Pair, RowSnapshot};
    use super::*;

    fn row(object_id: &str, name: &str, status: &str, bid: i64) -> DeltaRow {
        DeltaRow {
            object_id: object_id.to_owned(),
            name: name.to_owned(),
            epoch: 5,
            snapshot: RowSnapshot {
                rev: 1,
                dver: 1,
                fields: vec![
                    Pair {
                        field: "status".to_owned(),
                        kind: "string".to_owned(),
                        value: status.to_owned(),
                    },
                    Pair {
                        field: "high_bid".to_owned(),
                        kind: "integer".to_owned(),
                        value: bid.to_string(),
                    },
                ],
                failed: None,
            },
            tombstone: false,
        }
    }

    fn manifest() -> Manifest {
        Manifest {
            generation: 1,
            dver: 1,
            min_dver: 1,
            fields: vec![
                Field {
                    name: "high_bid".to_owned(),
                    kind: "integer".to_owned(),
                    since: 1,
                },
                Field {
                    name: "status".to_owned(),
                    kind: "string".to_owned(),
                    since: 1,
                },
            ],
            ..Default::default()
        }
    }

    struct Built {
        overlay: Overlay,
        _dir: tempfile::TempDir,
    }

    fn build(rows: &[DeltaRow], manifest: &Manifest) -> Built {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = delta::encode(rows, None, dir.path()).expect("encodes");
        let overlay = Overlay::build(
            Some(&bytes),
            NO_DELTAS,
            manifest,
            &dir.path().join("overlay.sqlite"),
            dir.path(),
        )
        .expect("builds");
        Built { overlay, _dir: dir }
    }

    fn names(page: &Page) -> Vec<&str> {
        page.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    #[test]
    fn a_predicate_selects_and_an_order_sorts() {
        let built = build(
            &[
                row("a", "lot-a", "open", 30),
                row("b", "lot-b", "shut", 10),
                row("c", "lot-c", "open", 20),
            ],
            &manifest(),
        );

        let page = built
            .overlay
            .list(
                &Query {
                    where_: Where(vec![Condition::Compare {
                        field: "status".into(),
                        op: Compare::Eq,
                        value: Value::Text("open".into()),
                    }]),
                    order: vec![Order {
                        field: "high_bid".to_owned(),
                        descending: true,
                    }],
                    limit: 10,
                    cursor: None,
                },
                &manifest(),
            )
            .expect("lists");
        assert_eq!(names(&page), vec!["lot-a", "lot-c"]);
        assert!(page.cursor.is_none(), "a short page is the last");
    }

    #[test]
    fn an_empty_query_lists_every_instance() {
        let built = build(
            &[row("a", "lot-a", "open", 1), row("b", "lot-b", "shut", 2)],
            &manifest(),
        );
        let page = built
            .overlay
            .list(
                &Query {
                    limit: 10,
                    ..Default::default()
                },
                &manifest(),
            )
            .expect("lists");
        assert_eq!(names(&page), vec!["lot-a", "lot-b"]);
    }

    #[test]
    fn a_cursor_walks_every_row_exactly_once() {
        let rows: Vec<DeltaRow> = (0..5)
            .map(|i| row(&format!("o{i}"), &format!("lot-{i}"), "open", i))
            .collect();
        let built = build(&rows, &manifest());

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = built
                .overlay
                .list(
                    &Query {
                        limit: 2,
                        cursor: cursor.clone(),
                        ..Default::default()
                    },
                    &manifest(),
                )
                .expect("lists");
            seen.extend(page.entries.iter().map(|entry| entry.name.clone()));
            match page.cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen, vec!["lot-0", "lot-1", "lot-2", "lot-3", "lot-4"]);
    }

    #[test]
    fn a_tombstoned_object_is_not_listed() {
        let mut dead = row("b", "lot-b", "open", 1);
        dead.tombstone = true;
        dead.snapshot.fields.clear();
        let built = build(&[row("a", "lot-a", "open", 1), dead], &manifest());

        let page = built
            .overlay
            .list(
                &Query {
                    limit: 10,
                    ..Default::default()
                },
                &manifest(),
            )
            .expect("lists");
        assert_eq!(names(&page), vec!["lot-a"]);
    }

    /// Values are stored as the kind the manifest names, not as text.
    /// Stored as text, sqlite compares them as text: 75 > 100 would be
    /// true because "75" > "100" lexicographically, and ordering would
    /// be wrong the same way.
    #[test]
    fn numbers_compare_and_sort_as_numbers() {
        let built = build(
            &[
                row("a", "lot-a", "open", 75),
                row("b", "lot-b", "open", 100),
                row("c", "lot-c", "open", 250),
            ],
            &manifest(),
        );

        let page = built
            .overlay
            .list(
                &Query {
                    where_: Where(vec![Condition::Compare {
                        field: "high_bid".into(),
                        op: Compare::Gt,
                        value: Value::Integer(100),
                    }]),
                    limit: 10,
                    ..Default::default()
                },
                &manifest(),
            )
            .expect("lists");
        assert_eq!(names(&page), vec!["lot-c"], "75 is not greater than 100");

        let page = built
            .overlay
            .list(
                &Query {
                    order: vec![Order {
                        field: "high_bid".to_owned(),
                        descending: true,
                    }],
                    limit: 10,
                    ..Default::default()
                },
                &manifest(),
            )
            .expect("lists");
        assert_eq!(names(&page), vec!["lot-c", "lot-b", "lot-a"]);
    }

    #[test]
    fn a_building_field_refuses_rather_than_answering_partially() {
        let mut manifest = manifest();
        // A field seen but not yet backfilled: rows derived before it
        // simply lack it, so answering would miss objects silently.
        manifest.fields.push(Field {
            name: "closes_at".to_owned(),
            kind: "integer".to_owned(),
            since: 2,
        });
        manifest.dver = 2;

        let built = build(&[row("a", "lot-a", "open", 1)], &manifest);
        let error = built
            .overlay
            .list(
                &Query {
                    where_: Where(vec![Condition::Compare {
                        field: "closes_at".into(),
                        op: Compare::Gt,
                        value: Value::Integer(0),
                    }]),
                    limit: 10,
                    ..Default::default()
                },
                &manifest,
            )
            .expect_err("a building field is refused");
        assert!(error.contains("building"), "{error}");

        // Everything already built still answers.
        assert!(
            built
                .overlay
                .list(
                    &Query {
                        limit: 10,
                        ..Default::default()
                    },
                    &manifest
                )
                .is_ok()
        );
    }

    #[test]
    fn a_later_delta_overwrites_an_older_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base =
            delta::encode(&[row("a", "lot-a", "open", 1)], None, dir.path()).expect("encodes");
        let mut newer = row("a", "lot-a", "shut", 9);
        newer.snapshot.rev = 2;
        let fresh = delta::encode(&[newer], None, dir.path()).expect("encodes");
        // Applied twice: the overlay absorbs unfolded deltas, and a
        // retry can deliver one more than once.
        let overlay = Overlay::build(
            Some(&base),
            &[fresh.clone(), fresh],
            &manifest(),
            &dir.path().join("overlay.sqlite"),
            dir.path(),
        )
        .expect("builds");

        let page = overlay
            .list(
                &Query {
                    limit: 10,
                    ..Default::default()
                },
                &manifest(),
            )
            .expect("lists");
        assert_eq!(page.entries.len(), 1);
        assert_eq!(
            page.entries[0].fields,
            vec![
                ("high_bid".to_owned(), Value::Integer(9)),
                ("status".to_owned(), Value::Text("shut".to_owned())),
            ]
        );
    }
}
