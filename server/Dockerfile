# Build stage
FROM rust:1.76-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin tunly-server

# Run stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/tunly-server /usr/local/bin/tunly-server
EXPOSE 3010
CMD ["tunly-server"]
