# Stage 1: Build the application
FROM rust:1.85 AS builder

# Set the working directory
WORKDIR /app

# Copy the source code of the app
COPY ./ ./

# Install OpenSSL 3 development headers (available in Bookworm environment)
RUN apt-get update && apt-get install -y \
    libssl-dev pkg-config && apt-get clean

# Build the application in release mode, linking it to OpenSSL 3
RUN cargo build --release

# Stage 2: Create the final runtime container
FROM debian:bookworm-slim

# Install runtime dependencies, including OpenSSL 3
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 && apt-get clean

# Set the working directory
WORKDIR /app

# Copy the built binary and the config from the previous stage
COPY --from=builder /app/target/release/igra-rpc-provider .
COPY --from=builder /app/config.toml .

# Set the entrypoint to the built binary
ENTRYPOINT ["./igra-rpc-provider"]