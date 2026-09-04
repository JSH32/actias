//! Reserved names the platform keeps for itself.

/// Kv namespaces starting with this prefix belong to the platform, so a
/// script cannot declare one. Nothing occupies the prefix today; holding
/// it is what lets the platform take a namespace later without breaking
/// a script that got there first, and it matches the same reservation on
/// object classes ([`crate::classes`]), the `__actias_` tables inside an
/// object's storage, and the worker's `_` url namespace.
pub const RESERVED_NAMESPACE_PREFIX: &str = "__";

/// Longest a region token may be.
pub const REGION_MAX_LEN: usize = 16;

/// Whether `region` is a region token: one to sixteen of `a-z`, `0-9`
/// and `-`, not starting with `-`. What `REGION` may hold.
pub fn is_region_token(region: &str) -> bool {
    let mut chars = region.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    region.len() <= REGION_MAX_LEN
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Why a name is refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NameError {
    #[error("An object name is a non-empty string.")]
    Empty,
}

/// Checks a name a caller chose or is holding: non-empty. A name is the
/// author's and never encodes where its object lives, so this is the
/// whole rule.
///
/// # Errors
/// Returns which rule the name broke.
pub fn validate_name(name: &str) -> Result<(), NameError> {
    if name.trim().is_empty() {
        return Err(NameError::Empty);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_names_pass_and_an_empty_one_is_refused() {
        assert_eq!(validate_name("lot-42"), Ok(()));
        assert_eq!(validate_name("cron:0 0 * * *"), Ok(()));
        assert_eq!(validate_name("orders/2026"), Ok(()));
        assert_eq!(validate_name("@anything"), Ok(()));
        assert_eq!(validate_name(""), Err(NameError::Empty));
        assert_eq!(validate_name("  "), Err(NameError::Empty));
    }

    #[test]
    fn region_tokens() {
        for ok in ["local", "eu-west", "us-east-1", "a", "0"] {
            assert!(is_region_token(ok), "{ok}");
        }
        for bad in ["", "-eu", "EU", "eu_west", "eu.west", "abcdefghijklmnopq"] {
            assert!(!is_region_token(bad), "{bad}");
        }
    }
}
