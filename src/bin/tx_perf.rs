use clap::Parser;
use igra_rpc_provider::tools::tx_perf::metrics::RpcSample;
use igra_rpc_provider::tools::tx_perf::metrics::{CumulativeMetrics, SlidingWindowMetrics};
use igra_rpc_provider::tools::tx_perf::parser::{parse_line, ParsedEvent};
use igra_rpc_provider::tools::tx_perf::printer::{format_cumulative, format_window, OutputFormat};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader as AsyncBufReader};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(
    name = "tx_perf",
    version,
    about = "Transaction submission performance from logs"
)]
struct Args {
    /// Read logs from a file instead of stdin
    #[arg(long)]
    input: Option<PathBuf>,

    /// Follow the input file (tail -f). Only applicable with --input.
    #[arg(long, default_value_t = false)]
    follow: bool,

    /// Summary interval in seconds
    #[arg(long = "summary-interval", default_value_t = 60)]
    summary_interval_secs: u64,

    /// Sliding window length in seconds
    #[arg(long = "window", default_value_t = 300)]
    window_secs: u64,

    /// Output format: text or md
    #[arg(long = "format", default_value = "text")]
    format: String,

    /// Show dropped unparsable line count (printed to stderr)
    #[arg(long = "show-dropped", default_value_t = false)]
    show_dropped: bool,

    /// Number of slowest RPC successes to display
    #[arg(long = "top-slowest", default_value_t = 5)]
    top_slowest: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let fmt = match args.format.as_str() {
        "text" => OutputFormat::Text,
        "md" | "markdown" => OutputFormat::Markdown,
        other => {
            eprintln!(
                "Unknown format '{}', supported: text|md. Falling back to text.",
                other
            );
            OutputFormat::Text
        }
    };
    let summary_interval = Duration::from_secs(args.summary_interval_secs);
    let window = Duration::from_secs(args.window_secs);

    // Startup banner for UX
    let source_desc = match (&args.input, args.follow) {
        (Some(path), follow) => format!("file:{} (follow={})", path.display(), follow),
        (None, _) => "stdin".to_string(),
    };
    let fmt_name = match fmt {
        OutputFormat::Text => "text",
        OutputFormat::Markdown => "md",
    };
    println!(
        "tx_perf started · source={} · interval={}s · window={}s · format={} · top_slowest={} · show_dropped={}",
        source_desc,
        args.summary_interval_secs,
        args.window_secs,
        fmt_name,
        args.top_slowest,
        args.show_dropped
    );

    let (tx, mut rx) = mpsc::channel::<String>(10_000);
    let reader_handle = match (&args.input, args.follow) {
        (Some(path), follow) => {
            let path = path.clone();
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                read_file_lines(path, follow, tx_clone).await;
            })
        }
        (None, _) => {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                read_stdin_lines(tx_clone).await;
            })
        }
    };
    drop(tx); // close sender when reader tasks stop

    // Optional SIGTERM support (Unix)
    #[cfg(unix)]
    let (mut term_rx, _term_handle) = {
        use tokio::signal::unix::{signal, SignalKind};
        let (tx, rx) = mpsc::channel::<()>(1);
        let handle = tokio::spawn(async move {
            if let Ok(mut s) = signal(SignalKind::terminate()) {
                let _ = s.recv().await;
                let _ = tx.send(()).await;
            }
        });
        (rx, handle)
    };

    let mut sliding = SlidingWindowMetrics::new(window);
    let mut cumulative = CumulativeMetrics::new(args.top_slowest);
    let mut dropped: u64 = 0;
    let mut interval = tokio::time::interval(summary_interval);
    // Avoid immediate tick at t=0
    interval.tick().await;

    // UX: one-time hint that we're waiting for data
    let mut printed_no_data_hint = false;

    let mut eof = false;
    loop {
        // Unix build: include SIGTERM branch
        #[cfg(unix)]
        {
            tokio::select! {
                biased;
                // Handle signals for graceful final summary
                _ = tokio::signal::ctrl_c() => {
                    print_final(&sliding, &cumulative, fmt, args.window_secs, args.top_slowest, args.show_dropped, dropped);
                    break;
                }
                _ = term_rx.recv() => {
                    print_final(&sliding, &cumulative, fmt, args.window_secs, args.top_slowest, args.show_dropped, dropped);
                    break;
                }
                // Periodic summary
                _ = interval.tick() => {
                    let now = Instant::now();
                    sliding.purge_expired(now);
                    if let Some(win) = sliding.summarize(args.top_slowest) {
                        let out = format_window(&win, args.window_secs, fmt, args.top_slowest);
                        println!("{}", out.trim_end());
                        if let Some(overall) = cumulative.summarize() {
                            let out2 = format_cumulative(&overall, fmt);
                            println!("{}", out2.trim_end());
                        }
                        if args.show_dropped && dropped > 0 {
                            eprintln!("Dropped unparsable lines: {}", dropped);
                            dropped = 0;
                        }
                    } else if !printed_no_data_hint {
                        if args.show_dropped && dropped > 0 {
                            println!(
                                "No RPC successes yet in last {}s… waiting for data (dropped={})",
                                args.window_secs, dropped
                            );
                            dropped = 0;
                        } else {
                            println!(
                                "No RPC successes yet in last {}s… waiting for data",
                                args.window_secs
                            );
                        }
                        printed_no_data_hint = true;
                    }
                }
                // Incoming log lines
                maybe_line = rx.recv() => {
                    match maybe_line {
                        Some(line) => {
                            if let Some(ev) = parse_line(&line) {
                                match ev {
                                    ParsedEvent::MiningSuccess { duration_micros } => {
                                        sliding.add_mining_success(duration_micros);
                                        cumulative.record_mining_success(duration_micros);
                                    }
                                    ParsedEvent::RpcSuccess { id, hash, latency_micros } => {
                                        sliding.add_rpc_success(RpcSample {
                                            at: Instant::now(),
                                            latency_micros,
                                            id: Some(id.clone()),
                                            hash: Some(hash.clone()),
                                        });
                                        cumulative.record_success(latency_micros, Some(id), Some(hash));
                                    }
                                    ParsedEvent::ProcessorError { error, .. } => {
                                        sliding.add_error(error.clone());
                                        cumulative.record_error();
                                    }
                                    ParsedEvent::WalletError { error, .. } => {
                                        sliding.add_error(error.clone());
                                        cumulative.record_error();
                                    }
                                    ParsedEvent::Capacity { capacity } => {
                                        sliding.add_capacity(capacity);
                                        cumulative.record_capacity(capacity);
                                    }
                                    ParsedEvent::WalletOkTime { .. } => {
                                        // Optional stage timing; not aggregated in v1
                                    }
                                    ParsedEvent::ProcessorOkTime { .. } => {
                                        // Optional stage timing; not aggregated in v1
                                    }
                                }
                            } else {
                                dropped = dropped.saturating_add(1);
                            }
                        }
                        None => {
                            // Sender closed (EOF and no follow)
                            eof = true;
                        }
                    }
                }
            }
        }
        // Non-Unix build: no SIGTERM branch
        #[cfg(not(unix))]
        {
            tokio::select! {
                biased;
                // Handle signals for graceful final summary
                _ = tokio::signal::ctrl_c() => {
                    print_final(&sliding, &cumulative, fmt, args.window_secs, args.top_slowest, args.show_dropped, dropped);
                    break;
                }
                // Periodic summary
                _ = interval.tick() => {
                    let now = Instant::now();
                    sliding.purge_expired(now);
                    if let Some(win) = sliding.summarize(args.top_slowest) {
                        let out = format_window(&win, args.window_secs, fmt, args.top_slowest);
                        println!("{}", out.trim_end());
                        if let Some(overall) = cumulative.summarize() {
                            let out2 = format_cumulative(&overall, fmt);
                            println!("{}", out2.trim_end());
                        }
                        if args.show_dropped && dropped > 0 {
                            eprintln!("Dropped unparsable lines: {}", dropped);
                            dropped = 0;
                        }
                    } else if !printed_no_data_hint {
                        if args.show_dropped && dropped > 0 {
                            println!(
                                "No RPC successes yet in last {}s… waiting for data (dropped={})",
                                args.window_secs, dropped
                            );
                            dropped = 0;
                        } else {
                            println!(
                                "No RPC successes yet in last {}s… waiting for data",
                                args.window_secs
                            );
                        }
                        printed_no_data_hint = true;
                    }
                }
                // Incoming log lines
                maybe_line = rx.recv() => {
                    match maybe_line {
                        Some(line) => {
                            if let Some(ev) = parse_line(&line) {
                                match ev {
                                    ParsedEvent::MiningSuccess { duration_micros } => {
                                        sliding.add_mining_success(duration_micros);
                                        cumulative.record_mining_success(duration_micros);
                                    }
                                    ParsedEvent::RpcSuccess { id, hash, latency_micros } => {
                                        sliding.add_rpc_success(igra_rpc_provider::tools::tx_perf::metrics::RpcSample {
                                            at: Instant::now(),
                                            latency_micros,
                                            id: Some(id.clone()),
                                            hash: Some(hash.clone()),
                                        });
                                        cumulative.record_success(latency_micros, Some(id), Some(hash));
                                    }
                                    ParsedEvent::ProcessorError { error, .. } => {
                                        sliding.add_error(error.clone());
                                        cumulative.record_error();
                                    }
                                    ParsedEvent::WalletError { error, .. } => {
                                        sliding.add_error(error.clone());
                                        cumulative.record_error();
                                    }
                                    ParsedEvent::Capacity { capacity } => {
                                        sliding.add_capacity(capacity);
                                        cumulative.record_capacity(capacity);
                                    }
                                    ParsedEvent::WalletOkTime { .. } => {
                                        // Optional stage timing; not aggregated in v1
                                    }
                                    ParsedEvent::ProcessorOkTime { .. } => {
                                        // Optional stage timing; not aggregated in v1
                                    }
                                }
                            } else {
                                dropped = dropped.saturating_add(1);
                            }
                        }
                        None => {
                            // Sender closed (EOF and no follow)
                            eof = true;
                        }
                    }
                }
            }
        }

        if eof {
            print_final(
                &sliding,
                &cumulative,
                fmt,
                args.window_secs,
                args.top_slowest,
                args.show_dropped,
                dropped,
            );
            break;
        }
    }

    // Wait for the reader to finish
    let _ = reader_handle.await;
    Ok(())
}

fn print_final(
    sliding: &SlidingWindowMetrics,
    cumulative: &CumulativeMetrics,
    fmt: OutputFormat,
    window_secs: u64,
    top_n: usize,
    show_dropped: bool,
    dropped: u64,
) {
    if let Some(win) = sliding.summarize(top_n) {
        let out = format_window(&win, window_secs, fmt, top_n);
        println!("{}", out.trim_end());
    }
    if let Some(overall) = cumulative.summarize() {
        let out2 = format_cumulative(&overall, fmt);
        println!("{}", out2.trim_end());
    }
    if show_dropped && dropped > 0 {
        eprintln!("Dropped unparsable lines: {}", dropped);
    }
}

async fn read_stdin_lines(tx: mpsc::Sender<String>) {
    let stdin = tokio::io::stdin();
    let mut reader = AsyncBufReader::new(stdin);
    let mut buf = String::new();
    loop {
        buf.clear();
        match reader.read_line(&mut buf).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                if let Err(_e) = tx.send(buf.trim_end_matches('\n').to_string()).await {
                    break;
                }
            }
            Err(_e) => break,
        }
    }
}

async fn read_file_lines(path: PathBuf, follow: bool, tx: mpsc::Sender<String>) {
    // Use blocking file IO within a blocking task because async file tailing is tricky
    let _ = tokio::task::spawn_blocking(move || {
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => return, // cannot open file
        };
        let mut reader = BufReader::new(file);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => {
                    if follow {
                        // Sleep briefly and try again
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        continue;
                    } else {
                        break;
                    }
                }
                Ok(_) => {
                    let line = buf.trim_end_matches('\n').to_string();
                    // Blocking send is not available; drop if full
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }
    })
    .await;
}
