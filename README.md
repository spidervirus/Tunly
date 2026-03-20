# 🚀 Tunly

**Tunly** is a modern, lightweight, and easy-to-self-host **ngrok alternative** written in Rust. It allows you to expose your local development servers to the internet securely via WebSocket-based tunneling.

[![Rust](https://img.shields.io/badge/rust-v1.76+-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## ✨ Features
- **Extreme Simplicity**: Just one command to expose your local port.
- **WebSocket-based**: High-performance HTTP tunneling (inspired by Chisel).
- **Auto-HTTPS**: Built-in Caddy sidecar for automatic Let's Encrypt SSL.
- **Auto-Reconnect**: Robust client with exponential backoff.
- **Custom Subdomains**: Random assignment or `--name` for reserved subdomains.

## 🚀 Quickstart

### 1. Server (Self-Host on your VPS)
Ensure you have Docker and Docker Compose installed. Point your domain (e.g., `*.tunly.sh` and `tunly.sh`) to your server's IP.

```bash
# Clone the repo
git clone https://github.com/spidervirus/tunly.git
cd tunly

# Set your domain and token
export TUNLY_DOMAIN=yourdomain.com
export TUNLY_TOKEN=mysecrettoken

# Start the server
docker compose up -d
```

### 2. Client (CLI)
```bash
# Compile the client
cargo build --release -p tunly-client

# Register a single tunnel (random name)
./target/release/tunly-client http 3000 --server wss://yourdomain.com/tunnel --token mysecrettoken

# Register with a custom name
./target/release/tunly-client http 3000 --server wss://yourdomain.com/tunnel --token mysecrettoken --name myapp

# Register multiple tunnels in one process
./target/release/tunly-client http 3000:api 4000:admin --server wss://yourdomain.com/tunnel --token mysecrettoken
```
Now access your site at: `https://myapp.yourdomain.com`

## 🛠 Local Dev Mode

To test **Tunly** locally without a real domain:

1. **Edit /etc/hosts**:
   Add the following lines to map your local fake domain and a subdomain:
   ```bash
   127.0.0.1  tunly.local
   127.0.0.1  myapp.tunly.local
   ```
2. **Start Server**:
   ```bash
   export TUNLY_DOMAIN=tunly.local
   docker compose up -d
   ```
3. **Run Client**:
   Use the `--insecure` flag to accept the self-signed certificate from Caddy:
   ```bash
   ./target/release/tunly-client 3000 \
     --server wss://tunly.local/tunnel \
     --token secret \
     --name myapp \
     --insecure
   ```
4. **Visit**: `https://myapp.tunly.local` (accept the browser security warning).

## 📊 Comparison

| Feature | **Tunly** | ngrok | frp | chisel | rathole | bore | zgrok |
|---------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| **Language** | Rust 🦀 | Go | Go | Go | Rust 🦀 | Rust 🦀 | Rust 🦀 |
| **Simplicity** | 🟢 Extreme | 🟢 High | 🔴 Low | 🟡 Medium | 🔴 Low | 🟢 High | 🟡 Medium |
| **Self-Host** | 🟢 1-Click | 🔴 SaaS | 🟡 Complex | 🟢 Simple | 🟡 Complex | 🟢 Simple | 🟢 Simple |
| **Auto-HTTPS** | 🟢 Yes | 🟢 Yes | 🔴 No | 🔴 No | 🔴 No | 🔴 No | 🟢 Yes |
| **Multi-Tunnel** | 🟢 Yes | 🟡 Paid | 🟢 Yes | 🟢 Yes | 🟢 Yes | 🔴 No | 🟢 Yes |
| **Streaming** | 🟢 Chunked | 🟢 Yes | 🟡 Mixed | 🟡 Mixed | 🟡 Mixed | 🔴 No | 🟢 Yes |
| **Auth** | 🟢 Token | 🟢 Complex | 🟡 Basic | 🟢 SSH | 🟡 Basic | 🔴 None | 🟢 Complex |

## 🛠 Self-Hosting Guide

Exposing your local machine to the internet shouldn't be hard. Tunly is designed to be the easiest self-hosted option.

### Prerequisites
- A VPS (DigitalOcean, Hetzner, AWS, etc.)
- A domain name (e.g., `tunly.sh`)
- Docker & Docker Compose

### 1. DNS Configuration
Set up two `A` records pointing to your VPS IP:
- `yourdomain.com` -> `IP`
- `*.yourdomain.com` -> `IP` (Wildcard is required for dynamic subdomains)

### 2. Server Deployment
```bash
# Clone and enter
git clone https://github.com/yourusername/tunly.git && cd tunly

# Set your variables
export TUNLY_DOMAIN=yourdomain.com
export TUNLY_TOKEN=choose_a_strong_token

# Launch with one command
docker compose up -d
```

Tunly uses **Caddy** as a sidecar. It will automatically:
1. Provisions TLS certificates from Let's Encrypt.
2. Terminate SSL for your main domain.
3. Dynamically route traffic to active tunnels based on headers.

### 3. Client Usage
Download the binary for your platform (see Releases) or build from source:

```bash
# Build (requires Rust)
cargo build --release -p tunly-client

# Simple tunnel
./tunly-client http 8000 --server wss://yourdomain.com/tunnel --token your_token

# Multiple tunnels in one go
./tunly-client http 3000:web 4000:api 5000:worker --token your_token
```

## 🛡 License
MIT
