//! The platform's built-in object class names, shared wherever an object
//! identity crosses a service boundary. The `__` prefix is reserved at
//! declaration time, so scripts can never collide with these; the worker
//! implements them, the script-service resolves owners for them, and the
//! api lists resources by them.

/// The built-in class behind `queue "name"`: its sqlite is the message
/// store, its alarm loop is the delivery loop.
pub const QUEUE_CLASS: &str = "__queue";

/// The built-in class behind `database "name"`: the sql product face over
/// object storage.
pub const DATABASE_CLASS: &str = "__database";

/// The built-in class behind `on "cron:<expr>"`: one instance per cron
/// event, scoped to its script rather than the project.
pub const CRON_CLASS: &str = "__cron";
