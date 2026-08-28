//! The one spelling of a duration: how scripts write time, wherever
//! they write it.

/// A duration written the way scripts write them: "500ms", "30s", "10m",
/// "24h", "7d", or a bare number of seconds.
///
/// # Errors
/// Returns text naming the malformed spelling.
pub fn parse_duration_ms(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    if let Ok(seconds) = raw.parse::<f64>() {
        return Ok((seconds * 1000.0) as i64);
    }

    let split = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .ok_or_else(|| format!("'{raw}' is not a duration."))?;
    let (number, unit) = raw.split_at(split);
    let number: f64 = number
        .parse()
        .map_err(|_| format!("'{raw}' is not a duration."))?;

    let factor = match unit.trim() {
        "ms" => 1.0,
        "s" => 1000.0,
        "m" => 60.0 * 1000.0,
        "h" => 3600.0 * 1000.0,
        "d" => 86400.0 * 1000.0,
        other => return Err(format!("Unknown duration unit '{other}'.")),
    };

    Ok((number * factor) as i64)
}
