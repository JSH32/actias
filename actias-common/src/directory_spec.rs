//! The one spelling of a declared directory field set.
//!
//! A class's publish declares which fields its directory exposes and
//! what kind each holds. That declaration crosses three crates: the
//! declaration pass writes it into the contract, the script-service
//! stamps the declaration version onto it, and the worker parses it
//! back out. One codec here, so the spelling cannot fork.
//!
//! The contract entry reads:
//!
//! ```text
//! Auction:directory@2=high_bid:integer,state:string,tags:array
//! ```
//!
//! Fields are sorted by name so the payload is canonical: two publishes
//! of the same set produce byte-identical entries, which is what lets
//! the version minting compare strings instead of parsing.

/// The kinds a declared field may hold. The single source: the
/// declaration pass validates against this, and the worker's typed
/// storage binds by these same names.
pub const FIELD_KINDS: [&str; 5] = ["string", "integer", "number", "boolean", "array"];

/// Names the query grammar owns, so no field may take them: `any`,
/// `all` and `none` are combinators and `name` is the instance itself.
pub const RESERVED_NAMES: [&str; 4] = ["any", "all", "none", "name"];

/// Longest field name, bytes. Names travel in every durable layer, so
/// the bound is on the name itself rather than on a column identifier.
pub const FIELD_NAME_MAX_BYTES: usize = 128;

/// Whether `name` may be a directory field, and why not when it may
/// not. Shared so the declaration pass refuses at publish exactly what
/// the kernel would refuse at derivation; two spellings of one rule is
/// how a class gets published and then fails on its first write.
///
/// # Errors
/// Names the offending field and the rule it broke.
pub fn check_field_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a directory field name cannot be empty".to_owned());
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!(
            "'{name}' is reserved and cannot be a directory field"
        ));
    }
    if name.len() > FIELD_NAME_MAX_BYTES {
        return Err(format!(
            "directory field '{name}' is longer than {FIELD_NAME_MAX_BYTES} bytes"
        ));
    }
    // The codec separates fields with `,` and name from kind with `:`,
    // and nested tables flatten to dotted names. Anything else in a
    // name would make an entry ambiguous to parse back.
    if name.contains(',') || name.contains(':') || name.contains('=') {
        return Err(format!(
            "directory field '{name}' may not contain ',', ':' or '='"
        ));
    }
    Ok(())
}

/// The marker between the class prefix and the payload.
const MARKER: &str = ":directory";

/// One declared field set, ordered and versioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectorySpec {
    /// The declaration version, minted at publish: bumps only when the
    /// field set actually changes, so an ordinary deploy renumbers
    /// nothing. Zero never appears in a contract; it is the version of
    /// rows derived before any publish declared fields.
    pub dver: u64,
    /// `(name, kind)`, sorted by name.
    pub fields: Vec<(String, String)>,
}

impl DirectorySpec {
    /// A spec from parsed parts, canonicalized.
    pub fn new(dver: u64, mut fields: Vec<(String, String)>) -> Self {
        fields.sort_by(|left, right| left.0.cmp(&right.0));
        Self { dver, fields }
    }

    /// The payload alone (`high_bid:integer,state:string`), canonical.
    /// Two equal field sets encode identically, whatever order they
    /// were declared in.
    pub fn payload(&self) -> String {
        self.fields
            .iter()
            .map(|(name, kind)| format!("{name}:{kind}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The full contract entry for `class`.
    pub fn entry(&self, class: &str) -> String {
        format!("{class}{MARKER}@{}={}", self.dver, self.payload())
    }

    /// Reads a contract entry back, answering `(class, spec)`.
    /// [`None`] for entries about anything else, including the bare
    /// pre-field marker `Class:directory`, which old contracts carry
    /// and which means "a directory exists, fields unknown".
    pub fn parse(entry: &str) -> Option<(String, DirectorySpec)> {
        let marker = entry.find(MARKER)?;
        let class = &entry[..marker];
        let rest = &entry[marker + MARKER.len()..];
        let rest = rest.strip_prefix('@')?;
        let (dver, payload) = rest.split_once('=')?;
        let dver = dver.parse().ok()?;

        let mut fields = Vec::new();
        for part in payload.split(',').filter(|part| !part.is_empty()) {
            let (name, kind) = part.split_once(':')?;
            fields.push((name.to_owned(), kind.to_owned()));
        }
        Some((class.to_owned(), DirectorySpec::new(dver, fields)))
    }

    /// Whether an entry names a directory for `class` at all, fielded
    /// or bare. What a console needs to offer the listing link.
    pub fn is_for(entry: &str, class: &str) -> bool {
        entry
            .strip_prefix(class)
            .and_then(|rest| rest.strip_prefix(MARKER))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('@'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_round_trips_and_is_canonical() {
        let spec = DirectorySpec::new(
            2,
            vec![
                ("state".to_owned(), "string".to_owned()),
                ("high_bid".to_owned(), "integer".to_owned()),
            ],
        );
        let entry = spec.entry("Auction");
        // Sorted by name whatever the declaration order, so equal sets
        // compare equal as strings.
        assert_eq!(entry, "Auction:directory@2=high_bid:integer,state:string");
        let (class, parsed) = DirectorySpec::parse(&entry).expect("parses");
        assert_eq!(class, "Auction");
        assert_eq!(parsed, spec);
    }

    #[test]
    fn the_bare_marker_is_not_a_spec_but_is_a_directory() {
        assert!(DirectorySpec::parse("Auction:directory").is_none());
        assert!(DirectorySpec::is_for("Auction:directory", "Auction"));
        assert!(DirectorySpec::is_for(
            "Auction:directory@2=state:string",
            "Auction"
        ));
        assert!(!DirectorySpec::is_for("Auction:expire=30d", "Auction"));
        assert!(!DirectorySpec::is_for(
            "Auctioneer:directory@1=x:string",
            "Auction"
        ));
    }

    #[test]
    fn an_empty_field_list_round_trips() {
        // Refused earlier by the declaration pass; the codec itself
        // stays total so a parse failure always means a malformed
        // entry, never a legal-but-empty one.
        let spec = DirectorySpec::new(1, Vec::new());
        let (_, parsed) = DirectorySpec::parse(&spec.entry("C")).expect("parses");
        assert!(parsed.fields.is_empty());
    }
}
