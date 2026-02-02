use igra_rpc_provider::{
    config::{
        AppConfig, GasConfig, MiningConfig, ProxyConfig, RetryConfig, SecurityConfig, ServerConfig,
        WalletConfig,
    },
    services::{
        gas_price::GasPriceService,
        proxy::ProxyService,
        transaction::{self, start_transaction_processor, WalletBackend},
    },
    types::rpc::RpcRequest,
    AppState,
};
use proto::kaswallet_proto::wallet_server::{Wallet, WalletServer};
use proto::kaswallet_proto::{
    signed_transaction, BroadcastRequest, BroadcastResponse, CreateUnsignedTransactionsRequest,
    CreateUnsignedTransactionsResponse, GetAddressesRequest, GetAddressesResponse, GetBalanceRequest,
    GetBalanceResponse, GetUtxosRequest, GetUtxosResponse, GetVersionRequest, GetVersionResponse,
    NewAddressRequest, NewAddressResponse, SendRequest, SendResponse, SignRequest, SignResponse,
    SignedTransaction, SignableTransaction, Transaction, TransactionOutput, WalletSignableTransaction,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};
use tonic::transport::Server;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KASWALLET_PASSWORD_ENV_VAR: &str = "KASWALLET_PASSWORD";

#[derive(Default)]
struct BroadcastGate {
    started: AtomicUsize,
    notify: tokio::sync::Notify,
}

impl BroadcastGate {
    fn arrive(&self) {
        let started = self.started.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        if started >= 2 {
            self.notify.notify_waiters();
        }
    }

    async fn wait_for_two(&self) -> Result<(), Status> {
        let deadline = Duration::from_secs(10);

        let wait_future = async {
            loop {
                if self.started.load(Ordering::SeqCst) >= 2 {
                    return Ok(());
                }

                let notified = self.notify.notified();
                if self.started.load(Ordering::SeqCst) >= 2 {
                    return Ok(());
                }

                notified.await;
            }
        };

        timeout(deadline, wait_future)
            .await
            .map_err(|_| Status::deadline_exceeded("timed out waiting for parallel broadcasts"))?
    }
}

#[derive(Clone)]
struct TestWallet {
    name: &'static str,
    gate: Arc<BroadcastGate>,
    create_calls: Arc<AtomicUsize>,
    sign_calls: Arc<AtomicUsize>,
    broadcast_calls: Arc<AtomicUsize>,
}

impl TestWallet {
    fn new(name: &'static str, gate: Arc<BroadcastGate>) -> Self {
        Self {
            name,
            gate,
            create_calls: Arc::new(AtomicUsize::new(0)),
            sign_calls: Arc::new(AtomicUsize::new(0)),
            broadcast_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[tonic::async_trait]
impl Wallet for TestWallet {
    async fn get_addresses(
        &self,
        _request: Request<GetAddressesRequest>,
    ) -> Result<Response<GetAddressesResponse>, Status> {
        Ok(Response::new(GetAddressesResponse { address: vec![] }))
    }

    async fn new_address(
        &self,
        _request: Request<NewAddressRequest>,
    ) -> Result<Response<NewAddressResponse>, Status> {
        Ok(Response::new(NewAddressResponse {
            address: "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h"
                .to_string(),
        }))
    }

    async fn get_balance(
        &self,
        _request: Request<GetBalanceRequest>,
    ) -> Result<Response<GetBalanceResponse>, Status> {
        Ok(Response::new(GetBalanceResponse {
            available: 0,
            pending: 0,
            address_balances: vec![],
        }))
    }

    async fn get_utxos(
        &self,
        _request: Request<GetUtxosRequest>,
    ) -> Result<Response<GetUtxosResponse>, Status> {
        Ok(Response::new(GetUtxosResponse {
            addresses_to_utxos: vec![],
        }))
    }

    async fn create_unsigned_transactions(
        &self,
        request: Request<CreateUnsignedTransactionsRequest>,
    ) -> Result<Response<CreateUnsignedTransactionsResponse>, Status> {
        self.create_calls.fetch_add(1, Ordering::SeqCst);

        let request = request.into_inner();
        let payload = request
            .transaction_description
            .map(|desc| desc.payload)
            .unwrap_or_default();

        let tx = Transaction {
            version: 0,
            inputs: vec![],
            outputs: vec![TransactionOutput {
                value: 1,
                script_public_key: None,
            }],
            lock_time: 0,
            subnetwork_id: Vec::new().into(),
            gas: 0,
            payload,
            mass: 0,
            id: Vec::new().into(),
        };

        let signable = SignableTransaction {
            tx: Some(tx),
            entries: vec![],
            calculated_fee: None,
            calculated_non_contextual_masses: None,
        };

        let wst = WalletSignableTransaction {
            transaction: Some(SignedTransaction {
                signed: Some(signed_transaction::Signed::Partially(signable)),
            }),
            derivation_paths: vec![],
            address_by_input_index: vec![],
            address_by_output_index: vec![],
        };

        Ok(Response::new(CreateUnsignedTransactionsResponse {
            unsigned_transactions: vec![wst],
        }))
    }

    async fn sign(&self, request: Request<SignRequest>) -> Result<Response<SignResponse>, Status> {
        self.sign_calls.fetch_add(1, Ordering::SeqCst);
        let request = request.into_inner();
        Ok(Response::new(SignResponse {
            signed_transactions: request.unsigned_transactions,
        }))
    }

    async fn broadcast(
        &self,
        _request: Request<BroadcastRequest>,
    ) -> Result<Response<BroadcastResponse>, Status> {
        self.broadcast_calls.fetch_add(1, Ordering::SeqCst);

        self.gate.arrive();
        self.gate.wait_for_two().await?;

        Ok(Response::new(BroadcastResponse {
            transaction_ids: vec![format!("{}-tx-1", self.name)],
        }))
    }

    async fn send(&self, _request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        Err(Status::unimplemented("Send is not used by this test server"))
    }

    async fn get_version(
        &self,
        _request: Request<GetVersionRequest>,
    ) -> Result<Response<GetVersionResponse>, Status> {
        Ok(Response::new(GetVersionResponse {
            version: "test".to_string(),
        }))
    }
}

struct WalletServerHandle {
    uri: String,
    create_calls: Arc<AtomicUsize>,
    sign_calls: Arc<AtomicUsize>,
    broadcast_calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

async fn spawn_wallet_server(name: &'static str, gate: Arc<BroadcastGate>) -> WalletServerHandle {
    let service = TestWallet::new(name, gate);
    let create_calls = service.create_calls.clone();
    let sign_calls = service.sign_calls.clone();
    let broadcast_calls = service.broadcast_calls.clone();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind wallet server listener");
    let addr = listener
        .local_addr()
        .expect("Failed to get wallet server local_addr");
    let uri = format!("http://{addr}");

    let task = tokio::spawn(async move {
        let incoming = TcpListenerStream::new(listener);
        Server::builder()
            .add_service(WalletServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .expect("Wallet server failed");
    });

    WalletServerHandle {
        uri,
        create_calls,
        sign_calls,
        broadcast_calls,
        task,
    }
}

async fn connect_wallet_backend(wallet_daemon_uri: String) -> WalletBackend {
    let wallet_config = WalletConfig {
        wallet_daemon_uri,
        to_address: "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h"
            .to_string(),
    };

    let connect_deadline = Duration::from_secs(10);
    let connect_future = async {
        loop {
            match WalletBackend::connect(wallet_config.clone()).await {
                Ok(backend) => return backend,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    };

    timeout(connect_deadline, connect_future)
        .await
        .expect("Timed out connecting to test wallet backend")
}

#[tokio::test]
async fn multi_wallet_processor_fans_out_transactions_in_parallel() {
    std::env::set_var(KASWALLET_PASSWORD_ENV_VAR, "test-password");

    // Mock EL base fee responses used by the transaction processor fee validation.
    let el_server = MockServer::start().await;
    let mock_block_response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "baseFeePerGas": "0x3b9aca00"
        }
    });

    let expected_request = json!({
        "jsonrpc": "2.0",
        "method": "eth_getBlockByNumber",
        "params": ["latest", false],
        "id": 1
    });

    Mock::given(method("POST"))
        .and(path("/"))
        .and(body_json(&expected_request))
        .respond_with(ResponseTemplate::new(200).set_body_json(mock_block_response))
        .mount(&el_server)
        .await;

    // Spin up two independent wallet daemons and ensure that two separate
    // transactions can be broadcast concurrently (i.e., not serialized through one wallet).
    let gate = Arc::new(BroadcastGate::default());
    let wallet1 = spawn_wallet_server("wallet-1", gate.clone()).await;
    let wallet2 = spawn_wallet_server("wallet-2", gate.clone()).await;

    let wallet_backend_1 = connect_wallet_backend(wallet1.uri.clone()).await;
    let wallet_backend_2 = connect_wallet_backend(wallet2.uri.clone()).await;

    let config = AppConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8535,
        },
        proxy: ProxyConfig::with_el_url(el_server.uri()),
        wallet: WalletConfig {
            wallet_daemon_uri: wallet1.uri.clone(),
            to_address: "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h"
                .to_string(),
        },
        wallets: vec![WalletConfig {
            wallet_daemon_uri: wallet2.uri.clone(),
            to_address: "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h"
                .to_string(),
        }],
        security: SecurityConfig {
            enable_whitelist: false,
            read_only: false,
        },
        mining: MiningConfig::with_settings(vec![0x00], 10),
        gas: GasConfig::with_min_protocol_fee_per_gas_gwei(1),
        retry: RetryConfig::default(),
    };

    let gas_price_service = GasPriceService::new(config.gas.clone());
    let transaction_sender = start_transaction_processor(
        config.clone(),
        vec![wallet_backend_1, wallet_backend_2],
        gas_price_service.clone(),
    );

    let proxy_service = ProxyService::new(config.el_url().to_string(), gas_price_service);
    let state = Arc::new(AppState::new(config, transaction_sender, proxy_service));

    // Real signed tx fixtures (same as unit tests in src/services/transaction.rs).
    let legacy_tx_hex = "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";
    let eip1559_tx_hex = "02f8d7824bd8820b558601d1a94a20018601d1a94a200182bf68940000000000000000000000000000000000feedad80b8645f872f55000000000000000000000000000000000000000000000000000000000026337595a0dc7c603d4296b70f5422daa22482d4afb088b29c426b4c9ec5ef019715a11978688306685db4f631b116ed0eeae19876fc9da3f3517653c8b35dee36ee90c080a01d70b4425acf0c6089788788fc51c1c2ef4e3f18203a2653fd462d9fc16bc0bba06b21e05530fa4d4b4ea243d15b4afcb08728cfbe3b788fcbf11687f542fa446f";

    let req_1 = RpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "eth_sendRawTransaction".to_string(),
        params: json!([format!("0x{legacy_tx_hex}")]),
        id: json!(1),
    };

    let req_2 = RpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "eth_sendRawTransaction".to_string(),
        params: json!([format!("0x{eip1559_tx_hex}")]),
        id: json!(2),
    };

    let (res_1, res_2) = timeout(Duration::from_secs(30), async {
        tokio::join!(
            transaction::process_transaction(req_1, state.clone()),
            transaction::process_transaction(req_2, state.clone())
        )
    })
    .await
    .expect("Timed out processing two transactions");

    assert!(res_1.get("result").is_some(), "Expected tx1 to succeed");
    assert!(res_2.get("result").is_some(), "Expected tx2 to succeed");

    // The concurrency gate ensures that two broadcasts were in-flight at once.
    assert_eq!(gate.started.load(Ordering::SeqCst), 2);

    // Ensure both wallet daemons received traffic (i.e., the pool fanned out).
    assert_eq!(wallet1.create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(wallet2.create_calls.load(Ordering::SeqCst), 1);
    assert_eq!(wallet1.sign_calls.load(Ordering::SeqCst), 1);
    assert_eq!(wallet2.sign_calls.load(Ordering::SeqCst), 1);
    assert_eq!(wallet1.broadcast_calls.load(Ordering::SeqCst), 1);
    assert_eq!(wallet2.broadcast_calls.load(Ordering::SeqCst), 1);

    wallet1.task.abort();
    wallet2.task.abort();
}
