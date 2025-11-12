//! Output formatting for transaction performance metrics.
//!
//! Provides human-readable formatting of performance summaries in both
//! plain text and Markdown formats for different consumption scenarios:
//! - Text: Terminal output, logs
//! - Markdown: Documentation, reports, dashboards

use crate::tools::tx_perf::metrics::{CumulativeSummary, WindowSummary};
use std::fmt::Write;

/// Output format for metrics display.
#[derive(Clone, Copy, Debug)]
pub enum OutputFormat {
    /// Plain text format for terminal output
    Text,
    /// Markdown format for reports and documentation
    Markdown,
}

pub fn format_window(
    summary: &WindowSummary,
    window_secs: u64,
    fmt: OutputFormat,
    top_n: usize,
) -> String {
    match fmt {
        OutputFormat::Text => format_window_text(summary, window_secs, top_n),
        OutputFormat::Markdown => format_window_md(summary, window_secs, top_n),
    }
}

pub fn format_cumulative(summary: &CumulativeSummary, fmt: OutputFormat) -> String {
    match fmt {
        OutputFormat::Text => format_cumulative_text(summary),
        OutputFormat::Markdown => format_cumulative_md(summary),
    }
}

fn format_window_text(s: &WindowSummary, window_secs: u64, top_n: usize) -> String {
    let mut out = String::new();
    let lat = LatencyMs::from_window(s);

    let _ = writeln!(
        out,
        "Window={window_secs}s  success={}  errors={}",
        s.success, s.errors
    );
    let _ = writeln!(
        out,
        "RPC latency (ms): p10={}  p25={}  p50={}  p90={}  p95={}  p99={}  min={}  avg={}  max={}",
        lat.p10, lat.p25, lat.p50, lat.p90, lat.p95, lat.p99, lat.min, lat.avg, lat.max
    );
    let _ = writeln!(out, "TPS (1/p50): {:.2}", s.tps);

    // Mining section (optional)
    if let Some(_ms) = s.mining_success {
        if let (
            Some(mp10),
            Some(mp25),
            Some(mp50),
            Some(mp90),
            Some(mp95),
            Some(mp99),
            Some(mmin),
            Some(mmean),
            Some(mmax),
        ) = (
            s.mining_p10,
            s.mining_p25,
            s.mining_p50,
            s.mining_p90,
            s.mining_p95,
            s.mining_p99,
            s.mining_min,
            s.mining_mean,
            s.mining_max,
        ) {
            let _ = writeln!(
                out,
                "Mining time (ms): p10={}  p25={}  p50={}  p90={}  p95={}  p99={}  min={}  avg={}  max={}",
                micros_to_ms(mp10),
                micros_to_ms(mp25),
                micros_to_ms(mp50),
                micros_to_ms(mp90),
                micros_to_ms(mp95),
                micros_to_ms(mp99),
                micros_to_ms(mmin),
                {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let avg_ms = (mmean / 1000.0).round() as u64;
                    avg_ms
                },
                micros_to_ms(mmax)
            );
        }
    }

    if let (Some(min), Some(avg)) = (s.capacity_min, s.capacity_avg) {
        let _ = writeln!(out, "Capacity: min={min}  avg={avg}");
    }

    if !s.top_errors.is_empty() {
        let _ = writeln!(out, "Top errors: {}", format_top_errors(&s.top_errors));
    }

    if !s.slowest.is_empty() {
        let slow_parts: Vec<_> = s
            .slowest
            .iter()
            .map(|(id, hash, us)| {
                format!(
                    "id={} hash={} {}ms",
                    fmt_optional(id),
                    fmt_optional(hash),
                    micros_to_ms(*us)
                )
            })
            .collect();
        let _ = writeln!(out, "Slowest[{top_n}]: {}", slow_parts.join("; "));
    }

    out
}

fn format_window_md(s: &WindowSummary, window_secs: u64, top_n: usize) -> String {
    let lat = LatencyMs::from_window(s);
    let mut out = String::new();

    let _ = writeln!(out, "### Tx Performance (last {window_secs}s)");
    let _ = writeln!(
        out,
        "- Success: {} · Errors: {} · TPS (1/p50): {:.2}",
        s.success, s.errors, s.tps
    );
    let _ = writeln!(
        out,
        "- RPC latency (ms): p10 {} · p25 {} · p50 {} · p90 {} · p95 {} · p99 {} · min {} · avg {} · max {}",
        lat.p10, lat.p25, lat.p50, lat.p90, lat.p95, lat.p99, lat.min, lat.avg, lat.max
    );

    // Mining section (optional)
    if let Some(_ms) = s.mining_success {
        if let (
            Some(mp10),
            Some(mp25),
            Some(mp50),
            Some(mp90),
            Some(mp95),
            Some(mp99),
            Some(mmin),
            Some(mmean),
            Some(mmax),
        ) = (
            s.mining_p10,
            s.mining_p25,
            s.mining_p50,
            s.mining_p90,
            s.mining_p95,
            s.mining_p99,
            s.mining_min,
            s.mining_mean,
            s.mining_max,
        ) {
            let _ = writeln!(
                out,
                "- Mining time (ms): p10 {} · p25 {} · p50 {} · p90 {} · p95 {} · p99 {} · min {} · avg {} · max {}",
                micros_to_ms(mp10),
                micros_to_ms(mp25),
                micros_to_ms(mp50),
                micros_to_ms(mp90),
                micros_to_ms(mp95),
                micros_to_ms(mp99),
                micros_to_ms(mmin),
                {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let avg_ms = (mmean / 1000.0).round() as u64;
                    avg_ms
                },
                micros_to_ms(mmax)
            );
        }
    }

    append_capacity(&mut out, s.capacity_min, s.capacity_avg, "- ");

    if !s.top_errors.is_empty() {
        let _ = writeln!(out, "- Top errors: {}", format_top_errors(&s.top_errors));
    }

    if !s.slowest.is_empty() {
        let parts: Vec<_> = s
            .slowest
            .iter()
            .map(|(id, hash, us)| {
                format!(
                    "[{} {}] {}ms",
                    fmt_optional(id),
                    fmt_optional(hash),
                    micros_to_ms(*us)
                )
            })
            .collect();
        let _ = writeln!(out, "- Slowest[{top_n}]: {}", parts.join("; "));
    }

    out
}

fn format_cumulative_text(s: &CumulativeSummary) -> String {
    let lat = LatencyMs::from_cumulative(s);
    let mut out = String::new();

    let _ = writeln!(
        out,
        "Overall  success={}  errors={}  elapsed={}s",
        s.success, s.errors, s.elapsed_secs
    );
    let _ = writeln!(
        out,
        "RPC latency (ms): p10={}  p25={}  p50={}  p90={}  p95={}  p99={}  min={}  avg={}  max={}",
        lat.p10, lat.p25, lat.p50, lat.p90, lat.p95, lat.p99, lat.min, lat.avg, lat.max
    );
    let _ = writeln!(out, "TPS (1/p50): {:.2}", s.tps);

    // Mining section (optional)
    if let Some(_ms) = s.mining_success {
        if let (
            Some(mp10),
            Some(mp25),
            Some(mp50),
            Some(mp90),
            Some(mp95),
            Some(mp99),
            Some(mmin),
            Some(mmean),
            Some(mmax),
        ) = (
            s.mining_p10,
            s.mining_p25,
            s.mining_p50,
            s.mining_p90,
            s.mining_p95,
            s.mining_p99,
            s.mining_min,
            s.mining_mean,
            s.mining_max,
        ) {
            let _ = writeln!(
                out,
                "Mining time (ms): p10={}  p25={}  p50={}  p90={}  p95={}  p99={}  min={}  avg={}  max={}",
                micros_to_ms(mp10),
                micros_to_ms(mp25),
                micros_to_ms(mp50),
                micros_to_ms(mp90),
                micros_to_ms(mp95),
                micros_to_ms(mp99),
                micros_to_ms(mmin),
                {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let avg_ms = (mmean / 1000.0).round() as u64;
                    avg_ms
                },
                micros_to_ms(mmax)
            );
        }
    }

    if let (Some(min), Some(avg)) = (s.capacity_min, s.capacity_avg) {
        let _ = writeln!(out, "Capacity: min={min}  avg={avg}");
    }

    out
}

fn format_cumulative_md(s: &CumulativeSummary) -> String {
    let lat = LatencyMs::from_cumulative(s);
    let mut out = String::new();

    let _ = writeln!(out, "### Cumulative (since start)");
    let _ = writeln!(
        out,
        "- Success: {} · Errors: {} · Elapsed: {}s · TPS (1/p50): {:.2}",
        s.success, s.errors, s.elapsed_secs, s.tps
    );
    let _ = writeln!(
        out,
        "- RPC latency (ms): p10 {} · p25 {} · p50 {} · p90 {} · p95 {} · p99 {} · min {} · avg {} · max {}",
        lat.p10, lat.p25, lat.p50, lat.p90, lat.p95, lat.p99, lat.min, lat.avg, lat.max
    );

    // Mining section (optional)
    if let Some(_ms) = s.mining_success {
        if let (
            Some(mp10),
            Some(mp25),
            Some(mp50),
            Some(mp90),
            Some(mp95),
            Some(mp99),
            Some(mmin),
            Some(mmean),
            Some(mmax),
        ) = (
            s.mining_p10,
            s.mining_p25,
            s.mining_p50,
            s.mining_p90,
            s.mining_p95,
            s.mining_p99,
            s.mining_min,
            s.mining_mean,
            s.mining_max,
        ) {
            let _ = writeln!(
                out,
                "- Mining time (ms): p10 {} · p25 {} · p50 {} · p90 {} · p95 {} · p99 {} · min {} · avg {} · max {}",
                micros_to_ms(mp10),
                micros_to_ms(mp25),
                micros_to_ms(mp50),
                micros_to_ms(mp90),
                micros_to_ms(mp95),
                micros_to_ms(mp99),
                micros_to_ms(mmin),
                {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let avg_ms = (mmean / 1000.0).round() as u64;
                    avg_ms
                },
                micros_to_ms(mmax)
            );
        }
    }

    append_capacity(&mut out, s.capacity_min, s.capacity_avg, "- ");

    out
}

/// Helper struct to hold pre-converted millisecond values for formatting.
struct LatencyMs {
    p10: u64,
    p25: u64,
    p50: u64,
    p90: u64,
    p95: u64,
    p99: u64,
    min: u64,
    max: u64,
    avg: u64,
}

impl LatencyMs {
    /// Convert microsecond latencies to milliseconds for display (window summary).
    fn from_window(s: &WindowSummary) -> Self {
        Self {
            p10: micros_to_ms(s.p10),
            p25: micros_to_ms(s.p25),
            p50: micros_to_ms(s.p50),
            p90: micros_to_ms(s.p90),
            p95: micros_to_ms(s.p95),
            p99: micros_to_ms(s.p99),
            min: micros_to_ms(s.min),
            max: micros_to_ms(s.max),
            avg: {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let avg_ms = (s.mean / 1000.0).round() as u64;
                avg_ms
            },
        }
    }

    /// Convert microsecond latencies to milliseconds for display (cumulative summary).
    fn from_cumulative(s: &CumulativeSummary) -> Self {
        Self {
            p10: micros_to_ms(s.p10),
            p25: micros_to_ms(s.p25),
            p50: micros_to_ms(s.p50),
            p90: micros_to_ms(s.p90),
            p95: micros_to_ms(s.p95),
            p99: micros_to_ms(s.p99),
            min: micros_to_ms(s.min),
            max: micros_to_ms(s.max),
            avg: {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let avg_ms = (s.mean / 1000.0).round() as u64;
                avg_ms
            },
        }
    }
}

/// Format top errors list as a comma-separated string.
fn format_top_errors(errors: &[(String, usize)]) -> String {
    errors
        .iter()
        .map(|(e, c)| format!("{e} ({c})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format optional value with fallback to "-".
fn fmt_optional(opt: &Option<String>) -> &str {
    opt.as_deref().unwrap_or("-")
}

/// Format capacity information if available.
fn append_capacity(
    out: &mut String,
    capacity_min: Option<u64>,
    capacity_avg: Option<u64>,
    separator: &str,
) {
    if let (Some(min), Some(avg)) = (capacity_min, capacity_avg) {
        let _ = writeln!(out, "{separator}Capacity: min {min} · avg {avg}");
    }
}

#[inline]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn micros_to_ms(us: u64) -> u64 {
    // Round to nearest ms
    // SAFETY: Duration values are always positive and within reasonable bounds for u64->f64->u64 conversion
    ((us as f64) / 1000.0).round() as u64
}
