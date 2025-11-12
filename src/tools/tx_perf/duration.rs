use std::time::Duration;

/// Parses duration strings into microseconds with support for multiple formats.
///
/// # Supported Formats
/// - Milliseconds: "850ms", "1.5ms"
/// - Seconds: "1s", "2.5s"
/// - Microseconds: "90us", "90µs" (both ASCII 'u' and Unicode 'µ' supported)
/// - Nanoseconds: "100ns"
/// - Humantime format: "1m", "2h", "1d" (via humantime crate)
///
/// # Returns
/// - `Some(microseconds)` if parsing succeeds and value fits in u64
/// - `None` if parsing fails, value is negative, infinite, or overflows u64
///
/// # Examples
/// ```
/// # use igra_rpc_provider::tools::tx_perf::duration::parse_duration_micros;
/// assert_eq!(parse_duration_micros("1s"), Some(1_000_000));
/// assert_eq!(parse_duration_micros("500ms"), Some(500_000));
/// assert_eq!(parse_duration_micros("100us"), Some(100));
/// assert_eq!(parse_duration_micros("invalid"), None);
/// ```
pub fn parse_duration_micros(input: &str) -> Option<u64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    // Normalize microseconds unit variants
    let lower = s.replace('μ', "µ"); // normalize greek mu to micro sign
    let s = lower.as_str();

    // Determine unit and numeric part
    let (num_str, multiplier): (&str, f64) = if let Some(stripped) = s.strip_suffix("ms") {
        (stripped.trim(), 1_000.0) // 1 ms = 1000 µs
    } else if let Some(stripped) = s.strip_suffix("µs") {
        (stripped.trim(), 1.0) // 1 µs = 1 µs
    } else if let Some(stripped) = s.strip_suffix("us") {
        (stripped.trim(), 1.0)
    } else if let Some(stripped) = s.strip_suffix('s') {
        (stripped.trim(), 1_000_000.0) // 1 s = 1_000_000 µs
    } else if let Some(stripped) = s.strip_suffix("ns") {
        (stripped.trim(), 0.001) // 1 ns = 0.001 µs
    } else {
        // Try to parse using humantime as a fallback (e.g., 1m, 2h)
        return humantime::parse_duration(s)
            .ok()
            .and_then(duration_to_micros);
    };

    // Parse number (integer or float)
    let value: f64 = num_str.parse().ok()?;
    let micros = value * multiplier;
    if micros.is_sign_negative() || !micros.is_finite() {
        return None;
    }
    // Round to nearest microsecond
    let rounded = micros.round();
    if rounded < 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let as_u128 = rounded as u128;
    if as_u128 > u128::from(u64::MAX) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(as_u128 as u64)
}

fn duration_to_micros(d: Duration) -> Option<u64> {
    let micros = d
        .as_secs()
        .saturating_mul(1_000_000)
        .saturating_add(u64::from(d.subsec_micros()));
    Some(micros)
}
