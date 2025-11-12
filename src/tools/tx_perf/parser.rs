//! Log parser for transaction performance analysis.
//!
//! This module parses structured log lines from the RPC provider to extract
//! transaction timing data, errors, and queue capacity metrics.
//!
//! # Supported Log Patterns
//! - `RPC RESPONSE`: Successful transaction processing with end-to-end latency
//! - `TX_PROCESSOR`: Transaction processor success/failure events with timing
//! - `WALLET_CALL`: Wallet interaction results with send times
//! - Queue capacity: Available transaction queue capacity metrics

use crate::tools::tx_perf::duration::parse_duration_micros;
use once_cell::sync::Lazy;
use regex::Regex;

/// Represents a parsed event from transaction processing logs.
#[derive(Debug, Clone)]
pub enum ParsedEvent {
    /// Mining finished successfully; duration in microseconds
    MiningSuccess {
        duration_micros: u64,
    },
    RpcSuccess {
        id: String,
        hash: String,
        latency_micros: u64,
    },
    ProcessorError {
        id: String,
        hash: String,
        error: String,
    },
    WalletError {
        hash: String,
        error: String,
    },
    Capacity {
        capacity: u64,
    },
    // Additional stage timings (not yet aggregated)
    WalletOkTime {
        hash: String,
        send_micros: u64,
    },
    ProcessorOkTime {
        id: String,
        hash: String,
        time_micros: u64,
    },
}

/// Parse a single log line into one of the supported events.
/// Returns None if the line is not relevant or cannot be parsed.
pub fn parse_line(line: &str) -> Option<ParsedEvent> {
    // Mining success (provider or wallet logs)
    if let Some(ev) = parse_mining_success(line) {
        return Some(ev);
    }
    // RPC success
    if let Some(ev) = parse_rpc_success(line) {
        return Some(ev);
    }
    // TX queue capacity
    if let Some(ev) = parse_queue_capacity(line) {
        return Some(ev);
    }
    // TX_PROCESSOR success
    if let Some(ev) = parse_processor_ok(line) {
        return Some(ev);
    }
    // TX_PROCESSOR error
    if let Some(ev) = parse_processor_err(line) {
        return Some(ev);
    }
    // WALLET_CALL success
    if let Some(ev) = parse_wallet_ok(line) {
        return Some(ev);
    }
    // WALLET_CALL error
    if let Some(ev) = parse_wallet_err(line) {
        return Some(ev);
    }
    None
}

/// Try to parse mining success duration from provider/wallet logs.
fn parse_mining_success(line: &str) -> Option<ParsedEvent> {
    // Provider tracing logs (text or JSON-like), prefer ms, fallback to seconds
    if line.contains("mining_") {
        if let Some(ms) = extract_numeric_after_key(line, "mining_duration_ms") {
            let micros = (ms * 1_000.0).round();
            if micros.is_sign_positive() && micros.is_finite() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    return Some(ParsedEvent::MiningSuccess {
                        duration_micros: micros as u64,
                    });
                }
            }
        } else if let Some(sec) = extract_numeric_after_key(line, "mining_duration_seconds") {
            let micros = (sec * 1_000_000.0).round();
            if micros.is_sign_positive() && micros.is_finite() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    return Some(ParsedEvent::MiningSuccess {
                        duration_micros: micros as u64,
                    });
                }
            }
        }
    }
    // Wallet human-readable line: "Mining completed: {nonces} nonces in {DURATION}, ..."
    if let Some(dur) = extract_wallet_mining_duration(line) {
        if let Some(duration_micros) = parse_duration_micros(&dur) {
            return Some(ParsedEvent::MiningSuccess { duration_micros });
        }
    }
    None
}

/// Extract numeric value after a key for patterns like:
/// - key=123, key = 123
/// - "key": 123
///
///   Returns a float to allow fractional seconds or milliseconds.
fn extract_numeric_after_key(line: &str, key: &str) -> Option<f64> {
    let idx = line.find(key)?;
    let start = idx.saturating_add(key.len());
    if start >= line.len() {
        return None;
    }
    let after = &line[start..];
    // Skip separators and whitespace: '=', ':', spaces, quotes
    let mut i = 0usize;
    let bytes = after.as_bytes();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b' ' || b == b'\t' || b == b'=' || b == b':' || b == b'"' {
            #[allow(clippy::arithmetic_side_effects)]
            {
                i += 1;
            }
        } else {
            break;
        }
    }
    // Collect number [0-9\.]+
    let rest = &after[i..];
    let mut end = 0usize;
    for ch in rest.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            #[allow(clippy::arithmetic_side_effects)]
            {
                end += ch.len_utf8();
            }
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    rest[..end].parse::<f64>().ok()
}

/// Extract duration substring from wallet mining completion line.
/// Matches: "Mining completed: (\\d+) nonces in ([^,]+),"
fn extract_wallet_mining_duration(line: &str) -> Option<String> {
    if !line.contains("Mining completed:") || !line.contains(" nonces in ") {
        return None;
    }
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"Mining completed:\s+\d+\s+nonces in ([^,]+),"#)
            .expect("Invalid regex pattern")
    });
    let caps = RE.captures(line)?;
    let dur = caps.get(1)?.as_str().trim().to_string();
    if dur.is_empty() {
        None
    } else {
        Some(dur)
    }
}

fn parse_rpc_success(line: &str) -> Option<ParsedEvent> {
    // Example:
    // RPC RESPONSE [id={ID}, hash={HASH}]: Transaction processed successfully, time={DURATION}, payload_size={BYTES} bytes
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"RPC RESPONSE \[id=([^,\]]+), hash=([^\]]+)\]"#)
            .expect("Invalid regex pattern")
    });
    let caps = RE.captures(line)?;
    let id = caps.get(1)?.as_str().to_string();
    let hash = caps.get(2)?.as_str().to_string();
    let time = extract_time_value(line, "time")?;
    let latency_micros = parse_duration_micros(&time)?;
    Some(ParsedEvent::RpcSuccess {
        id,
        hash,
        latency_micros,
    })
}

fn parse_queue_capacity(line: &str) -> Option<ParsedEvent> {
    // Example:
    // TX [id={ID}, hash={HASH}]: Transaction queued successfully, queue_time={DURATION}, available_capacity={CAP}
    // Tolerate either available_capacity or capacity
    let capacity = extract_capacity_value(line)?;
    Some(ParsedEvent::Capacity { capacity })
}

fn parse_processor_ok(line: &str) -> Option<ParsedEvent> {
    // Example:
    // TX_PROCESSOR [id={ID}, hash={HASH}]: Transaction processed successfully, time={DURATION}, ...
    if !line.contains("TX_PROCESSOR") || !line.contains("processed successfully") {
        return None;
    }
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"TX_PROCESSOR \[id=([^,\]]+), hash=([^\]]+)\]"#)
            .expect("Invalid regex pattern")
    });
    let caps = RE.captures(line)?;
    let id = caps.get(1)?.as_str().to_string();
    let hash = caps.get(2)?.as_str().to_string();
    let time = extract_time_value(line, "time")?;
    let time_micros = parse_duration_micros(&time)?;
    Some(ParsedEvent::ProcessorOkTime {
        id,
        hash,
        time_micros,
    })
}

fn parse_processor_err(line: &str) -> Option<ParsedEvent> {
    // Example:
    // TX_PROCESSOR [id={ID}, hash={HASH}]: Transaction failed: {ERR}, time={DURATION}, ...
    if !line.contains("TX_PROCESSOR") || !line.contains("Transaction failed:") {
        return None;
    }
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"TX_PROCESSOR \[id=([^,\]]+), hash=([^\]]+)\]"#)
            .expect("Invalid regex pattern")
    });
    let caps = RE.captures(line)?;
    let id = caps.get(1)?.as_str().to_string();
    let hash = caps.get(2)?.as_str().to_string();
    // Extract error message (best-effort)
    let err = extract_error_message_after_prefix(line, "Transaction failed: ")?;
    Some(ParsedEvent::ProcessorError {
        id,
        hash,
        error: err,
    })
}

fn parse_wallet_ok(line: &str) -> Option<ParsedEvent> {
    // Example:
    // WALLET_CALL [hash={HASH}]: Transaction accepted by wallet, payload_size={BYTES}, send_time={DURATION}
    if !line.contains("WALLET_CALL") || !line.contains("accepted by wallet") {
        return None;
    }
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"WALLET_CALL \[hash=([^\]]+)\]"#).expect("Invalid regex pattern")
    });
    let caps = RE.captures(line)?;
    let hash = caps.get(1)?.as_str().to_string();
    let time = extract_time_value(line, "send_time")?;
    let send_micros = parse_duration_micros(&time)?;
    Some(ParsedEvent::WalletOkTime { hash, send_micros })
}

fn parse_wallet_err(line: &str) -> Option<ParsedEvent> {
    // Example:
    // WALLET_CALL [hash={HASH}]: Send failed: {ERR}, time={DURATION}
    if !line.contains("WALLET_CALL") || !line.contains("Send failed:") {
        return None;
    }
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"WALLET_CALL \[hash=([^\]]+)\]"#).expect("Invalid regex pattern")
    });
    let caps = RE.captures(line)?;
    let hash = caps.get(1)?.as_str().to_string();
    let err = extract_error_message_after_prefix(line, "Send failed: ")?;
    Some(ParsedEvent::WalletError { hash, error: err })
}

fn extract_time_value(line: &str, key: &str) -> Option<String> {
    // Fast non-regex scanner: key=<value> up to comma/space
    let needle = {
        #[allow(clippy::arithmetic_side_effects)]
        let mut s = String::with_capacity(key.len() + 1);
        s.push_str(key);
        s.push('=');
        s
    };
    let idx = line.find(&needle)?;
    let start = idx.saturating_add(needle.len());
    if start >= line.len() {
        return Some(String::new());
    }
    let bytes = line.as_bytes();
    let mut end = start;
    while end < bytes.len() {
        let b = bytes[end];
        if b == b',' || b.is_ascii_whitespace() {
            break;
        }
        #[allow(clippy::arithmetic_side_effects)]
        {
            end += 1;
        }
    }
    Some(line[start..end].to_string())
}

fn extract_error_message_after_prefix(line: &str, prefix: &str) -> Option<String> {
    let idx = line.find(prefix)?;
    let start = idx.saturating_add(prefix.len());
    if start >= line.len() {
        return None;
    }
    let after = &line[start..];
    // Stop at ", time=" if present, otherwise to end of line or comma
    if let Some(end) = after.find(", time=") {
        return Some(after[..end].trim().to_string());
    }
    if let Some(end) = after.find(",") {
        return Some(after[..end].trim().to_string());
    }
    Some(after.trim().to_string())
}

fn extract_capacity_value(line: &str) -> Option<u64> {
    if let Some(v) = extract_time_value(line, "available_capacity") {
        if let Ok(n) = v.parse::<u64>() {
            return Some(n);
        }
    }
    if let Some(v) = extract_time_value(line, "capacity") {
        if let Ok(n) = v.parse::<u64>() {
            return Some(n);
        }
    }
    None
}
