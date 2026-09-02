//! What a call on an object can fail with.

/// Why a call did not return a value.
#[derive(Debug)]
pub enum ObjectError {
    /// The method failed or does not exist; the text is the script's own
    /// error, exactly as a request handler's failure would read.
    Call(String),
    /// The method ran and committed, but the platform could not confirm
    /// the write had left the node before the gate's budget ran out. The
    /// outcome is unknown rather than failed: the frames are still being
    /// retried, so the call may yet become durable. A caller that
    /// retries needs the same idempotence a network timeout demands.
    NotDurable(String),
    /// The object's task is gone; the caller should resolve the object
    /// again rather than retry blindly.
    Gone,
}

impl std::fmt::Display for ObjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObjectError::Call(message) => f.write_str(message),
            ObjectError::NotDurable(message) => {
                write!(f, "The call's outcome is unknown: {message}.")
            }
            ObjectError::Gone => f.write_str("the object's task is gone"),
        }
    }
}

impl std::error::Error for ObjectError {}
