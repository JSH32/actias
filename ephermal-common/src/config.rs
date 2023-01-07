pub use dotenv::dotenv;

use std::{env, fmt::Debug, str::FromStr};

/// Get env as [`T`] or `default`.
pub fn get_env_or<T>(var: &str, default: T) -> T
where
    T: FromStr,
    <T as FromStr>::Err: Debug,
{
    match env::var(var) {
        Ok(v) => v.parse::<T>().expect(&format!(
            "Unable to parse {} as {}",
            var,
            std::any::type_name::<T>()
        )),
        Err(_) => default,
    }
}

/// Get env as [`T`].
pub fn get_env<T>(var: &str) -> T
where
    T: FromStr,
    <T as FromStr>::Err: Debug,
{
    env::var(var)
        .expect(&format!("Missing environment variable {}", var))
        .parse::<T>()
        .expect(&format!(
            "Unable to parse {} as {}",
            var,
            std::any::type_name::<T>()
        ))
}
