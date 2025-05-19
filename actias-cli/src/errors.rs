use std::fmt;

use colored::Colorize;

/// Custom error type for the application
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

/// Handle and print errors in a consistent way
pub fn print_error(err: &Error) {
    match err {
        Error::Authentication(msg) => println!("❌ {}: {}", "Authentication Error".red(), msg),
        Error::Permission(msg) => println!("❌ {}: {}", "Permission Error".red(), msg),
        Error::Api(msg) => println!("❌ {}: {}", "API Error".red(), msg),
        Error::Io(msg) => println!("❌ {}: {}", "IO Error".red(), msg),
        Error::Config(msg) => println!("❌ {}: {}", "Configuration Error".red(), msg),
        Error::Script(msg) => println!("❌ {}: {}", "Script Error".red(), msg),
        Error::Command(msg) => println!("❌ {}: {}", "Error".red(), msg),
        Error::NotFound(msg) => println!("❌ {}: {}", "Not Found".red(), msg),
    }
}

/// Result type alias for our custom error
pub type Result<T> = std::result::Result<T, Error>;
