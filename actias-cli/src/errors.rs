//! What a command can fail with, and the one shape every failure prints
//! in. Raw `Debug` never reaches the terminal.

use std::fmt;

/// Every way a cli command can fail, by the audience the message is for.
#[derive(Debug)]
pub enum Error {
    Authentication(String),
    Permission(String),
    Api(String),
    Io(String),
    Config(String),
    Script(String),
    Command(String),
    NotFound(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (prefix, message) = match self {
            Error::Authentication(msg) => ("Authentication Error", msg),
            Error::Permission(msg) => ("Permission Error", msg),
            Error::Api(msg) => ("API Error", msg),
            Error::Io(msg) => ("IO Error", msg),
            Error::Config(msg) => ("Configuration Error", msg),
            Error::Script(msg) => ("Script Error", msg),
            Error::Command(msg) => ("Command Error", msg),
            Error::NotFound(msg) => ("Not Found", msg),
        };

        write!(f, "{}: {}", prefix, message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error::Api(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Config(err.to_string())
    }
}

impl From<String> for Error {
    fn from(err: String) -> Self {
        Error::Command(err)
    }
}

/// Helper function to convert a progenitor API error to our custom error
pub fn progenitor_error<E: std::error::Error>(err: E) -> Error {
    Error::Api(err.to_string())
}

/// Prints one error to the terminal, in the one shape every command uses.
pub fn print_error(err: &Error) {
    match err {
        Error::Authentication(msg) => crate::ui::error("authentication error", msg),
        Error::Permission(msg) => crate::ui::error("permission denied", msg),
        Error::Api(msg) => crate::ui::error("api error", msg),
        Error::Io(msg) => crate::ui::error("io error", msg),
        Error::Config(msg) => crate::ui::error("configuration error", msg),
        Error::Script(msg) => crate::ui::error("script error", msg),
        Error::Command(msg) => crate::ui::error("error", msg),
        Error::NotFound(msg) => crate::ui::error("not found", msg),
    }
}

/// Result type alias for our custom error
pub type Result<T> = std::result::Result<T, Error>;
