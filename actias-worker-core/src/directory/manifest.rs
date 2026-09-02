//! The class's directory manifest: what a reader consults to find the
//! base, and the only mutable key in the whole layout.
//!
//! It also carries the answer to the question the field-set model turns
//! on. Fields are not declared anywhere: they exist once the class's
//! `directory` function has run against real state, so the manifest
//! learns them from the rows themselves and records, per field, the
//! generation it first appeared at. A field is queryable once every row
//! has been derived at or past that generation, which is one integer
//! comparison against [`Manifest::min_dver`].

use serde::{Deserialize, Serialize};

/// One field the class is known to expose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    /// The predicate family the field admits, taken from the values
    /// seen: an array field takes membership, a scalar takes
    /// comparisons.
    pub kind: String,
    /// The field-set generation this field first appeared at. Rows
    /// derived before it may lack the field entirely, which is why a
    /// query on it waits.
    pub since: u64,
}

/// What the store remembers about one class's directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "one")]
    pub version: u32,
    /// Bumps on every compaction that lays a new base; names the base
    /// generation a reader is looking at.
    #[serde(default)]
    pub generation: u64,
    /// The content-addressed base holding the merged rows, absent
    /// before the first compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Delta names already folded into `base`. A reader skips these and
    /// reads the rest; the compactor collects them once a later
    /// generation exists.
    #[serde(default)]
    pub folded: Vec<String>,
    /// The field-set generation. Bumps only when a field is first seen,
    /// so an ordinary deploy that changes no field leaves it alone and
    /// nothing rebuilds.
    #[serde(default)]
    pub dver: u64,
    /// Every field the class is known to expose.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// The lowest dver across every row in the base. A field is built
    /// when this reaches the generation the field appeared at, because
    /// every row was then derived under a declaration that produced it.
    #[serde(default)]
    pub min_dver: u64,
    /// Live rows in the base, for the building progress a console
    /// shows.
    #[serde(default)]
    pub rows: u64,
    /// The identity checksum of those live rows
    /// ([`super::identity::checksum`]), compared against the placement
    /// store's own fold to decide whether the class is intact.
    ///
    /// [`None`] on a manifest written before the checksum existed,
    /// which is not the same as a checksum of zero: absent means "not
    /// known", so the gate opens and the next fold records the real
    /// value, while zero is what an empty class legitimately folds to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identities: Option<i64>,
}

fn one() -> u32 {
    1
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            generation: 0,
            base: None,
            folded: Vec::new(),
            dver: 0,
            fields: Vec::new(),
            min_dver: 0,
            rows: 0,
            identities: None,
        }
    }
}

impl Manifest {
    /// Whether every row carries this field, so a query on it answers
    /// completely rather than missing objects that have not been
    /// re-derived.
    ///
    /// An unknown field is not building, it is absent: the caller
    /// surfaces that as the typo refusal, because a field nobody has
    /// ever produced is far more likely a misspelling than a wait.
    pub fn is_built(&self, field: &str) -> bool {
        self.field(field)
            .is_some_and(|known| self.min_dver >= known.since)
    }

    /// The fields a query must refuse for now, with the generation each
    /// is waiting to reach. Empty once everything is built.
    pub fn building(&self) -> Vec<&Field> {
        self.fields
            .iter()
            .filter(|field| self.min_dver < field.since)
            .collect()
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|known| known.name == name)
    }

    /// Folds one publish's declared field set in, and returns whether
    /// anything changed.
    ///
    /// A field's `since` is the version of the publish that introduced
    /// it, never the arrival order of whichever delta folded first.
    ///
    /// Deltas are an unordered bag, so declarations arrive in any
    /// order and this merge must converge regardless:
    ///
    /// - A newer declaration's set becomes the field set. A field the
    ///   previous set already had, at the same kind, keeps its `since`;
    ///   new or kind-changed fields date from the new version. Fields
    ///   it no longer declares are dropped: rows stop carrying them,
    ///   and a query on one becomes the unknown-field refusal.
    /// - An older declaration only lowers `since` where it proves a
    ///   field existed earlier than currently recorded, at the same
    ///   kind. It never adds or removes fields: the newest set won.
    /// - The same version changes nothing; equal versions carry equal
    ///   sets, because publish mints a new version whenever the set
    ///   differs.
    pub fn observe_declaration(
        &mut self,
        spec: &actias_common::directory_spec::DirectorySpec,
    ) -> bool {
        use std::cmp::Ordering;
        match spec.dver.cmp(&self.dver) {
            Ordering::Greater => {
                self.fields = spec
                    .fields
                    .iter()
                    .map(|(name, kind)| Field {
                        since: match self.field(name) {
                            Some(known) if known.kind == *kind => known.since,
                            _ => spec.dver,
                        },
                        name: name.clone(),
                        kind: kind.clone(),
                    })
                    .collect();
                self.fields
                    .sort_by(|left, right| left.name.cmp(&right.name));
                self.dver = spec.dver;
                true
            }
            Ordering::Less => {
                let mut changed = false;
                for (name, kind) in &spec.fields {
                    if let Some(known) = self
                        .fields
                        .iter_mut()
                        .find(|known| known.name == *name && known.kind == *kind)
                        && known.since > spec.dver
                    {
                        known.since = spec.dver;
                        changed = true;
                    }
                }
                changed
            }
            Ordering::Equal => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actias_common::directory_spec::DirectorySpec;

    fn spec(dver: u64, fields: &[(&str, &str)]) -> DirectorySpec {
        DirectorySpec::new(
            dver,
            fields
                .iter()
                .map(|(name, kind)| ((*name).to_owned(), (*kind).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn the_first_publish_is_queryable_once_its_rows_arrive() {
        let mut manifest = Manifest::default();
        assert!(manifest.observe_declaration(&spec(1, &[("status", "string"), ("tags", "array")])));
        assert_eq!(manifest.dver, 1);
        assert_eq!(manifest.field("status").unwrap().since, 1);

        // Rows derived under that publish carry its version; once the
        // floor reaches it, everything is built. No baseline special
        // case: `since` comes from the publish, never from which delta
        // happened to fold first.
        manifest.min_dver = 1;
        assert!(manifest.is_built("status"));
        assert!(manifest.building().is_empty());
    }

    #[test]
    fn an_ordinary_deploy_changes_nothing() {
        let mut manifest = Manifest::default();
        manifest.observe_declaration(&spec(1, &[("status", "string")]));
        let before = manifest.clone();

        // Publish mints a new version only when the set changes, so an
        // unchanged deploy re-offers the same spec: a no-op. This is
        // the case that has to stay free.
        assert!(!manifest.observe_declaration(&spec(1, &[("status", "string")])));
        assert_eq!(manifest, before);
    }

    #[test]
    fn a_new_field_waits_until_every_row_carries_it() {
        let mut manifest = Manifest::default();
        manifest.observe_declaration(&spec(1, &[("status", "string")]));
        manifest.min_dver = 1;
        assert!(manifest.is_built("status"));

        // The next publish adds a field: rows derived under version 1
        // may simply lack it, and a query then misses those objects
        // silently. So it waits for the floor.
        manifest.observe_declaration(&spec(2, &[("status", "string"), ("closes_at", "integer")]));
        assert_eq!(manifest.dver, 2);
        assert!(!manifest.is_built("closes_at"));
        assert!(
            manifest.is_built("status"),
            "an older field stays queryable while a newer one builds"
        );
        assert_eq!(
            manifest
                .building()
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["closes_at"]
        );

        // Backfill lifts the floor; the field opens.
        manifest.min_dver = 2;
        assert!(manifest.is_built("closes_at"));
        assert!(manifest.building().is_empty());
    }

    #[test]
    fn declarations_converge_whatever_order_the_deltas_arrive_in() {
        // The same two publishes, folded in both orders, land on the
        // same manifest: deltas are an unordered bag and this is the
        // property that makes that safe.
        let v1 = spec(1, &[("status", "string")]);
        let v2 = spec(2, &[("status", "string"), ("closes_at", "integer")]);

        let mut forward = Manifest::default();
        forward.observe_declaration(&v1);
        forward.observe_declaration(&v2);

        let mut backward = Manifest::default();
        backward.observe_declaration(&v2);
        backward.observe_declaration(&v1);

        assert_eq!(forward.fields, backward.fields);
        assert_eq!(forward.dver, backward.dver);
        assert_eq!(
            forward.field("status").unwrap().since,
            1,
            "the older publish proves status existed at 1, whichever order arrived"
        );
        assert_eq!(forward.field("closes_at").unwrap().since, 2);
    }

    #[test]
    fn a_field_that_changes_kind_rebuilds() {
        let mut manifest = Manifest::default();
        manifest.observe_declaration(&spec(1, &[("tags", "string")]));
        manifest.min_dver = 1;

        assert!(manifest.observe_declaration(&spec(2, &[("tags", "array")])));
        assert_eq!(manifest.field("tags").unwrap().kind, "array");
        assert!(
            !manifest.is_built("tags"),
            "old rows hold values the new predicates cannot ask about"
        );
        assert_eq!(
            manifest.fields.len(),
            1,
            "the field is replaced, not doubled"
        );
    }

    #[test]
    fn a_dropped_field_leaves_the_manifest() {
        let mut manifest = Manifest::default();
        manifest.observe_declaration(&spec(1, &[("status", "string"), ("draft", "boolean")]));

        assert!(manifest.observe_declaration(&spec(2, &[("status", "string")])));
        assert!(
            manifest.field("draft").is_none(),
            "rows stop carrying it; a query on it is the unknown-field refusal"
        );
        assert_eq!(
            manifest.field("status").unwrap().since,
            1,
            "kept fields keep their history"
        );
    }

    #[test]
    fn an_unknown_field_is_absent_rather_than_building() {
        let manifest = Manifest::default();
        assert!(!manifest.is_built("statsu"));
        assert!(
            manifest.building().is_empty(),
            "a field nobody produced is a typo, not a wait"
        );
    }

    #[test]
    fn a_manifest_from_before_a_field_still_reads() {
        let bare: Manifest = serde_json::from_str(r#"{ "generation": 3 }"#).expect("parses");
        assert_eq!(bare.generation, 3);
        assert_eq!(bare.version, 1);
        assert!(bare.fields.is_empty());
        assert!(bare.base.is_none());
    }
}
