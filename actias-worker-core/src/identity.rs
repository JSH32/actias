//! Object identity: the (scope, class, name) triple every durable object
//! is addressed by, and the forms derived from it (the hashed platform id,
//! the storage file name).
//!
//! The scope encodes the platform's sharing rule in one place: resource
//! classes (queues, databases, user object classes) scope to the project,
//! so every script in a project addressing `queue "jobs"` reaches one
//! object; `__cron` scopes to its script, because a schedule belongs to
//! the script that declared it and equal expressions across a project must
//! never collide.

use crate::extensions::objects::CRON_CLASS;

/// One object's identity. Constructed through [`ObjectKey::scoped`] so the
/// scoping rule cannot be applied differently at different call sites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectKey {
    scope: String,
    class: String,
    name: String,
}

impl ObjectKey {
    /// The identity for a class instance, choosing the scope by the
    /// platform's sharing rule: the script for `__cron`, the project for
    /// everything else.
    pub fn scoped(project_id: &str, script_id: &str, class: &str, name: &str) -> Self {
        let scope = if class == CRON_CLASS {
            script_id
        } else {
            project_id
        };
        Self {
            scope: scope.to_owned(),
            class: class.to_owned(),
            name: name.to_owned(),
        }
    }

    /// An identity received over an internal transport, whose sender
    /// already chose the scope; never invents a scope of its own.
    pub fn received(scope: &str, class: &str, name: &str) -> Self {
        Self {
            scope: scope.to_owned(),
            class: class.to_owned(),
            name: name.to_owned(),
        }
    }

    /// Reads a key back from its canonical string form (chain entries,
    /// persisted alarm keys). [`None`] when the string is not a key.
    pub fn parse(key: &str) -> Option<Self> {
        let mut parts = key.splitn(3, '/');
        let scope = parts.next().filter(|s| !s.is_empty())?;
        let class = parts.next().filter(|s| !s.is_empty())?;
        let name = parts.next().filter(|s| !s.is_empty())?;
        Some(Self {
            scope: scope.to_owned(),
            class: class.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The identity scope: a project id, or the script id for `__cron`.
    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn class(&self) -> &str {
        &self.class
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this identity belongs to a script's schedule rather than a
    /// project resource.
    pub fn is_cron(&self) -> bool {
        self.class == CRON_CLASS
    }

    /// The platform-wide object id: blake3 of the canonical string, hex.
    /// The lease key in the placement store, the snapshot key in the blob
    /// store and the storage file stem, because class and instance names
    /// are user-chosen text.
    pub fn object_id(&self) -> String {
        blake3::hash(self.to_string().as_bytes())
            .to_hex()
            .to_string()
    }

    /// The SQLite file this object's state lives in, under a data dir.
    pub fn db_file_name(&self) -> String {
        format!("{}.db", self.object_id())
    }

    /// The region this object is born in: the class's pin, else the
    /// project's home. Computed from what the caller holds, never looked
    /// up, which is what keeps placement free of a global namespace; an
    /// object the platform has since moved is found by the forwarding
    /// row at this region (FLEET.md section 4.2).
    pub fn region<'a>(&'a self, home: &'a str, pin: Option<&'a str>) -> &'a str {
        pin.unwrap_or(home)
    }
}

impl std::fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.scope, self.class, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::objects::QUEUE_CLASS;

    #[test]
    fn resource_classes_scope_to_the_project_and_cron_to_its_script() {
        let queue = ObjectKey::scoped("proj-1", "script-1", QUEUE_CLASS, "jobs");
        assert_eq!(queue.scope(), "proj-1");

        let other_script = ObjectKey::scoped("proj-1", "script-2", QUEUE_CLASS, "jobs");
        assert_eq!(
            queue.object_id(),
            other_script.object_id(),
            "two scripts in one project share the queue"
        );

        let cron = ObjectKey::scoped("proj-1", "script-1", CRON_CLASS, "cron:*/5 * * * *");
        let cron_other = ObjectKey::scoped("proj-1", "script-2", CRON_CLASS, "cron:*/5 * * * *");
        assert_eq!(cron.scope(), "script-1");
        assert_ne!(
            cron.object_id(),
            cron_other.object_id(),
            "equal expressions across a project never collide"
        );
    }

    #[test]
    fn keys_round_trip_their_string_form() {
        let key = ObjectKey::scoped("proj-1", "script-1", CRON_CLASS, "cron:0 0 * * *");
        let parsed = ObjectKey::parse(&key.to_string()).expect("parses");
        assert_eq!(parsed, key);
        // The name is the tail, so user text with slashes survives.
        assert_eq!(parsed.name(), "cron:0 0 * * *");

        assert!(ObjectKey::parse("not-a-key").is_none());
    }

    #[test]
    fn the_birth_region_is_the_pins_then_the_homes() {
        let pinned = ObjectKey::scoped("proj-1", "script-1", "EuCustomer", "alice");
        assert_eq!(pinned.region("us-east", Some("eu-west")), "eu-west");

        let named = ObjectKey::scoped("proj-1", "script-1", "Auction", "lot-42");
        assert_eq!(named.region("us-east", None), "us-east");
        assert!(ObjectKey::parse("scope/class-only").is_none());
        assert!(ObjectKey::parse("//name").is_none());
    }

    #[test]
    fn the_file_name_is_the_hashed_id() {
        let key = ObjectKey::scoped("p", "s", QUEUE_CLASS, "jobs");
        assert_eq!(key.db_file_name(), format!("{}.db", key.object_id()));
        assert_eq!(key.object_id().len(), 64);
    }
}
