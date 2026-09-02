//! Reading a service's configuration from the environment. Every
//! service's `config.rs` builds on these two functions, so a missing
//! variable fails at startup rather than on a request.

pub use dotenv::dotenv;

use std::{env, fmt::Debug, str::FromStr};

/// Reads `var` from the environment as `T`, or `default` when unset.
pub fn get_env_or<T>(var: &str, default: T) -> T
where
    T: FromStr,
    <T as FromStr>::Err: Debug,
{
    match env::var(var) {
        Ok(v) => v.parse::<T>().unwrap_or_else(|_| {
            panic!("Unable to parse {} as {}", var, std::any::type_name::<T>())
        }),
        Err(_) => default,
    }
}

/// Reads `var` from the environment as `T`, panicking at startup
/// when it is unset or unparseable.
pub fn get_env<T>(var: &str) -> T
where
    T: FromStr,
    <T as FromStr>::Err: Debug,
{
    env::var(var)
        .unwrap_or_else(|_| panic!("Missing environment variable {}", var))
        .parse::<T>()
        .unwrap_or_else(|_| panic!("Unable to parse {} as {}", var, std::any::type_name::<T>()))
}
