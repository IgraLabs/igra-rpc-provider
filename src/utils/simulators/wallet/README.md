# Kaspa Wallet Simulator

The **Kaspa Wallet Simulator** is a standalone CLI utility designed to simulate sending raw transactions to the (IGRA version of the) KASPA Wallet. 
It can be used for testing purposes or interacting with an EL client that support the `eth_sendRawTransaction` JSON-RPC method.

## Features

- Sends raw transactions in hex format to an Ethereum-like JSON-RPC endpoint.
- Provides helpful error handling for invalid transactions or RPC communication issues.
- Easy to use as a command-line tool.

## How to Build the Simulator

To compile the simulator, navigate to the root of the project and run:

```bash
cargo build --release --bin kaspa_wallet_simulator
```

The resulting binary will be located in the `target/release` directory:

```bash
./target/release/kaspa_wallet_simulator
```

## Usage

Run the simulator using the following command:

```bash
cargo run --bin kaspa_wallet_simulator -- --raw-tx <RAW_TRANSACTION> --rpc-url <RPC_URL>
```

### Options:

- `--raw-tx`: The raw transaction in hex bytes, prefixed with `0x`.
- `--rpc-url`: The URL of the Ethereum-like JSON-RPC endpoint to which the raw transaction will be sent.

### Example Command:

```bash
cargo run --bin kaspa_wallet_simulator -- --raw-tx 0xabcdef123456 --rpc-url http://127.0.0.1:8545
```

Alternatively, if you have built the binary, use:

```bash
./target/release/kaspa_wallet_simulator --raw-tx 0xabcdef123456 --rpc-url http://127.0.0.1:8545
```

## Error Handling

The simulator will return descriptive error messages in the following cases:
- The `--raw-tx` does not start with `0x`.
- Connection to the RPC endpoint fails.
- The RPC endpoint returns an error or an unexpected response.

## License

This tool is part of the `igra-rpc-provider` project. See the main project’s license for details.