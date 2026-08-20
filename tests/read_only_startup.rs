//! Startup regressions for read-only mode.
//!
//! These drive the real binary rather than a library replica, because the behavior under test lives in
//! `main` and `AppConfig::load`: configuration validation, lane resolution placement, and whether the
//! wallet daemon is dialed at all. A router-level test with a hand-built `AppState` would prove such a
//! state can be served, not that startup produces it.

use std::net::TcpListener;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::time::{timeout, Instant};
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Every variable `AppConfig::load` maps onto a config path. The child must not inherit any of these
/// from a developer shell or CI runner, or the scenario under test is not the scenario that runs.
const CONFIG_ENV_VARS: &[&str] = &[
    "SERVER_HOST",
    "SERVER_PORT",
    "EL_URL",
    "EL_WS_URL",
    "PROXY_TIMEOUT_SECONDS",
    "PROXY_MAX_RETRIES",
    "PROXY_RETRY_DELAY_MS",
    "WALLET_DAEMON_URI",
    "WALLET_TO_ADDRESS",
    "SECURITY_ENABLE_WHITELIST",
    "READ_ONLY",
    "TX_ID_PREFIX",
    "MINING_TIMEOUT_SECONDS",
    "IGRA_LANE_ID",
    "LANE_ENFORCEMENT_DISABLED",
    "MIN_PROTOCOL_FEE_PER_GAS_GWEI",
    "RETRY_MAX_ATTEMPTS",
    "RETRY_INITIAL_DELAY_MS",
    "RETRY_MAX_DELAY_MS",
    "KASWALLET_PASSWORD",
];

const CHAIN_ID: &str = "0x14da";
const VALID_KASPA_ADDRESS: &str =
    "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// The provider binary, with a sanitized environment and the repository root as its working
/// directory so `File::with_name("config").required(true)` resolves the checked-in `config.toml`.
///
/// `kill_on_drop` means a panicking assertion cannot leave a server running: the `Child` is dropped
/// while unwinding and the process is killed with it.
///
/// `RUST_LOG` is set rather than merely removed: `EnvFilter::try_from_default_env()` reads it, and a
/// parent exporting `RUST_LOG=off` would otherwise suppress the log lines these tests assert on.
fn provider_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_igra-rpc-provider"));
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUST_LOG", "info")
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for key in CONFIG_ENV_VARS {
        command.env_remove(key);
    }

    command
}

/// Reserve a loopback port and release it. The child cannot be given port 0 directly because
/// `ServerConfig::validate` rejects zero, so the port has to be chosen up front.
fn reserve_port() -> u16 {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("should be able to bind an ephemeral port");
    let port = listener
        .local_addr()
        .expect("bound listener should have a local address")
        .port();
    drop(listener);
    port
}

/// Wait until the child accepts TCP connections. Readiness is deliberately probed on connect rather
/// than by sending RPC requests, so polling does not inflate the EL mock's request count.
async fn wait_until_serving(port: u16, child: &mut Child) {
    let started = Instant::now();

    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }

        // An early exit is the interesting failure, and it has two very different causes: the
        // startup regression this test exists to catch, or a lost race for the reserved port. Report
        // the child's own logs so they are not mistaken for each other.
        if let Ok(Some(status)) = child.try_wait() {
            let logs = collect_output(child).await;
            panic!("provider exited before serving ({status}); output:\n{logs}");
        }

        if started.elapsed() > STARTUP_TIMEOUT {
            panic!("provider did not start serving within {STARTUP_TIMEOUT:?}");
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drain whatever the child has written so far. Used only on the failure path.
async fn collect_output(child: &mut Child) -> String {
    let mut combined = String::new();

    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut combined).await;
    }
    if let Some(mut stderr) = child.stderr.take() {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer).await;
        combined.push_str(&buffer);
    }

    combined
}

async fn rpc_call(port: u16, rpc_method: &str, params: Value) -> Value {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://127.0.0.1:{port}"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": rpc_method,
            "params": params,
        }))
        .send()
        .await
        .unwrap_or_else(|err| panic!("{rpc_method} request should reach the provider: {err}"));

    response
        .json::<Value>()
        .await
        .unwrap_or_else(|err| panic!("{rpc_method} response should be JSON: {err}"))
}

/// An EL that answers `eth_chainId` and records everything it receives.
async fn start_mock_el() -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "eth_chainId" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": CHAIN_ID,
        })))
        .mount(&server)
        .await;

    server
}

/// The regression this ticket exists to prevent: with `READ_ONLY=true`, no wallet daemon, no wallet
/// settings and no lane id, the server must bind its listener and answer reads.
///
/// Before this change the process died twice over — first at config load on the missing
/// `IGRA_LANE_ID`, then, once past that, on dialing the wallet daemon.
#[tokio::test]
async fn read_only_serves_without_wallet_or_lane_configuration() {
    let mock_el = start_mock_el().await;
    let port = reserve_port();

    let mut child = provider_command()
        .env("READ_ONLY", "true")
        .env("SERVER_HOST", "127.0.0.1")
        .env("SERVER_PORT", port.to_string())
        .env("EL_URL", mock_el.uri())
        // Empty rather than absent: this is what makes the skipped wallet *validation* observable
        // here. With `config.toml`'s valid values the test would only cover the lane regression.
        .env("WALLET_DAEMON_URI", "")
        .env("WALLET_TO_ADDRESS", "")
        .spawn()
        .expect("provider binary should spawn");

    wait_until_serving(port, &mut child).await;

    // Reads are proxied to the EL, so answering at all proves the whole startup path completed.
    let chain_id = rpc_call(port, "eth_chainId", json!([])).await;
    assert_eq!(
        chain_id["result"], CHAIN_ID,
        "read-only server should proxy eth_chainId, got: {chain_id}"
    );

    // The read-only guard must still answer for itself, unchanged by the wallet refactor.
    let write = rpc_call(port, "eth_sendRawTransaction", json!(["0x00"])).await;
    assert_eq!(
        write["error"]["code"],
        json!(-32000),
        "write should be rejected with -32000, got: {write}"
    );
    assert_eq!(
        write["error"]["message"], "Read-only mode is enabled",
        "unexpected rejection message: {write}"
    );
    assert_eq!(write["id"], json!(1), "response should echo the request id");

    // The rejection must come from the gate, not from something downstream that already forwarded
    // the transaction. If this ever fails, a read-only node is broadcasting.
    let requests = mock_el
        .received_requests()
        .await
        .expect("mock EL should be recording requests");
    let forwarded_write = requests
        .iter()
        .any(|request| String::from_utf8_lossy(&request.body).contains("eth_sendRawTransaction"));
    assert!(
        !forwarded_write,
        "read-only mode forwarded a write method to the EL"
    );
}

/// A read-write deployment that cannot reach its wallet daemon must fail loudly.
///
/// It used to log the error and return `Ok(())`, so the process exited 0 and, under
/// `restart: unless-stopped`, crash-looped while looking like a clean shutdown to anything watching
/// exit codes. The exit code is asserted alongside the diagnostic, because a bare non-zero status
/// could just as easily come from an unrelated configuration failure.
#[tokio::test]
async fn read_write_exits_non_zero_when_wallet_is_unreachable() {
    // Reserve and release, so the address is refused rather than merely assumed to be closed.
    let refused_port = reserve_port();

    let child = provider_command()
        .env("READ_ONLY", "false")
        // Satisfies lane validation so the run reaches the wallet. Using the escape hatch inside a
        // read-write test does not revive it as a read-only workaround.
        .env("LANE_ENFORCEMENT_DISABLED", "true")
        .env(
            "WALLET_DAEMON_URI",
            format!("http://127.0.0.1:{refused_port}"),
        )
        .env("WALLET_TO_ADDRESS", VALID_KASPA_ADDRESS)
        .spawn()
        .expect("provider binary should spawn");

    let output = timeout(EXIT_TIMEOUT, child.wait_with_output())
        .await
        .expect("provider should exit rather than hang when the wallet is unreachable")
        .expect("should be able to collect provider output");

    assert_eq!(
        output.status.code(),
        Some(1),
        "unreachable wallet daemon should exit 1, got {:?}",
        output.status
    );

    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        logs.contains("Failed to create WalletCaller"),
        "exit should be attributable to wallet initialization, got: {logs}"
    );
}
