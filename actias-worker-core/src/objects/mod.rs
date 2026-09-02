//! The serialization primitive under durable objects: one long-lived vm
//! owned by one tokio task, every call a mailbox message answered through
//! a oneshot. The input gate is the mailbox loop itself: the next message
//! is popped only after the current handler has finished, so object code
//! never observes interleaved execution, even across await points.
//!
//! This is substrate: it knows how to own a vm and serialize calls into
//! it. What a class is, where state lives and who may call arrive in the
//! layers above.

use std::collections::HashMap;
use std::sync::Arc;

use mlua::LuaSerdeExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::runtime::ActiasRuntime;

mod error;
pub use error::*;
mod handle;
pub use handle::*;
mod home;
pub use home::*;
mod task;
pub use task::*;
mod host;
pub use host::*;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
