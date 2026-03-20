use clap::Parser;
use futures_util::{SinkExt, StreamExt, future};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{error, info, warn, info_span, Instrument};
use uuid::Uuid;
use url::Url;
use rand::Rng;
use dashmap::DashMap;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "wss://tunly.sh/tunnel")]
    server: String,

    #[arg(short, long)]
    token: String,

    #[arg(short, long)]
    insecure: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug, Clone)]
enum Commands {
    /// Tunnel HTTP traffic to a local port
    Http {
        /// Port or "port:name" (e.g. 3000 or 3000:api)
        #[arg(action = clap::ArgAction::Append, required = true)]
        tunnels: Vec<String>,
    },
    /// Tunnel TCP traffic (stub)
    Tcp {
        port: u16,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum TunnelMessage {
    Register { token: String, name: Option<String> },
    Registered { subdomain: String },
    
    RequestStart {
        id: Uuid,
        method: String,
        path: String,
        headers: std::collections::HashMap<String, String>,
    },
    RequestChunk { id: Uuid, body: String },
    RequestEnd { id: Uuid },

    ResponseStart {
        id: Uuid,
        status: u16,
        headers: std::collections::HashMap<String, String>,
    },
    ResponseChunk { id: Uuid, body: String },
    ResponseEnd { id: Uuid },

    Error { message: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Arc::new(Args::parse());

    match &args.command {
        Commands::Http { tunnels } => {
            let mut handles = vec![];
            for tunnel_spec in tunnels {
                let parts: Vec<&str> = tunnel_spec.split(':').collect();
                let port = parts[0].parse::<u16>()?;
                let name = parts.get(1).map(|&s| s.to_string());
                let args = args.clone();
                
                let handle = tokio::spawn(async move {
                    let mut attempt = 0;
                    let initial_backoff = Duration::from_secs(1);
                    let max_backoff = Duration::from_secs(60);

                    loop {
                        attempt += 1;
                        info!(attempt = attempt, port = port, "Connecting to server...");
                        
                        match run_client(&args, port, name.clone()).await {
                            Ok(_) => {
                                info!(port = port, "Connection closed gracefully. Reconnecting...");
                                attempt = 0;
                            }
                            Err(e) => {
                                let base = initial_backoff.as_secs_f64() * 2.0_f64.powi(attempt as i32 - 1);
                                let current_backoff = Duration::from_secs_f64(base.min(max_backoff.as_secs_f64()));
                                let jitter = rand::thread_rng().gen_range(0.0..current_backoff.as_secs_f64());
                                let sleep_duration = Duration::from_secs_f64(jitter);

                                error!(port = port, error = %e, "Connection error. Retrying in {:.2?}...", sleep_duration);
                                sleep(sleep_duration).await;
                            }
                        }
                    }
                });
                handles.push(handle);
            }
            let _ = future::join_all(handles).await;
            Ok(())
        }
        Commands::Tcp { .. } => {
            info!("TCP tunneling is not yet implemented (stub).");
            Ok(())
        }
    }
}

async fn run_client(args: &Args, port: u16, name: Option<String>) -> anyhow::Result<()> {
    let url = Url::parse(&args.server)?;

    let connector = if args.insecure {
        let mut builder = native_tls::TlsConnector::builder();
        builder.danger_accept_invalid_certs(true);
        builder.danger_accept_invalid_hostnames(true);
        Some(tokio_tungstenite::Connector::NativeTls(builder.build()?))
    } else {
        None
    };

    let (ws_stream_full, _) = tokio_tungstenite::connect_async_tls_with_config(url, None, false, connector).await?;
    
    // Step 1: Register
    let (mut ws_sink, mut ws_stream) = ws_stream_full.split();
    let reg = TunnelMessage::Register {
        token: args.token.clone(),
        name,
    };
    ws_sink.send(Message::Text(serde_json::to_string(&reg)?)).await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = ws_sink.send(msg).await {
                error!("WebSocket send error: {}", e);
                break;
            }
        }
    });

    let client = reqwest::Client::new();
    let local_base_url = format!("http://localhost:{}", port);
    let pending_requests = Arc::new(DashMap::<Uuid, mpsc::UnboundedSender<TunnelMessage>>::new());

    while let Some(msg) = ws_stream.next().await {
        let msg = msg?;
        if let Message::Text(text) = msg {
            let tunnel_msg: TunnelMessage = serde_json::from_str(&text)?;
            match tunnel_msg {
                TunnelMessage::Registered { subdomain } => {
                    info!(subdomain = %subdomain, "Tunnel registered successfully!");
                    let domain = Url::parse(&args.server)?.host_str().unwrap_or("yourdomain.com").to_string();
                    info!("🚀 Public URL: https://{}.{}", subdomain, domain);
                }
                TunnelMessage::RequestStart { id, method, path, headers } => {
                    let span = info_span!("request", id = %id, method = %method, path = %path);
                    let (request_tx, mut request_rx) = mpsc::unbounded_channel::<TunnelMessage>();
                    pending_requests.insert(id, request_tx);

                    let local_url = format!("{}{}", local_base_url, path);
                    let client = client.clone();
                    let tx = tx.clone();
                    let pending_requests = pending_requests.clone();

                    tokio::spawn(async move {
                        info!("Forwarding to local server: {}", local_url);
                        
                        let body_stream = async_stream::stream! {
                            while let Some(msg) = request_rx.recv().await {
                                match msg {
                                    TunnelMessage::RequestChunk { body, .. } => {
                                        if let Ok(data) = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, body) {
                                            yield Ok::<bytes::Bytes, core::convert::Infallible>(bytes::Bytes::from(data));
                                        }
                                    }
                                    TunnelMessage::RequestEnd { .. } => break,
                                    _ => (),
                                }
                            }
                        };

                        let mut req_builder = client.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), &local_url)
                            .body(reqwest::Body::wrap_stream(body_stream));
                        
                        for (k, v) in headers {
                            if k.to_lowercase() != "host" {
                                req_builder = req_builder.header(k, v);
                            }
                        }

                        match req_builder.send().await {
                            Ok(res) => {
                                let status = res.status().as_u16();
                                let mut res_headers = std::collections::HashMap::new();
                                for (name, value) in res.headers() {
                                    res_headers.insert(name.to_string(), value.to_str().unwrap_or("").to_string());
                                }
                                
                                let _ = tx.send(Message::Text(serde_json::to_string(&TunnelMessage::ResponseStart {
                                    id,
                                    status,
                                    headers: res_headers,
                                }).unwrap()));

                                let mut res_stream = res.bytes_stream();
                                while let Some(Ok(chunk)) = res_stream.next().await {
                                    let _ = tx.send(Message::Text(serde_json::to_string(&TunnelMessage::ResponseChunk {
                                        id,
                                        body: base64::Engine::encode(&base64::prelude::BASE64_STANDARD, chunk),
                                    }).unwrap()));
                                }

                                let _ = tx.send(Message::Text(serde_json::to_string(&TunnelMessage::ResponseEnd { id }).unwrap()));
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to reach local server");
                                let _ = tx.send(Message::Text(serde_json::to_string(&TunnelMessage::ResponseStart {
                                    id,
                                    status: 502,
                                    headers: std::collections::HashMap::new(),
                                }).unwrap()));
                                let _ = tx.send(Message::Text(serde_json::to_string(&TunnelMessage::ResponseChunk {
                                    id,
                                    body: base64::Engine::encode(&base64::prelude::BASE64_STANDARD, "Bad Gateway"),
                                }).unwrap()));
                                let _ = tx.send(Message::Text(serde_json::to_string(&TunnelMessage::ResponseEnd { id }).unwrap()));
                            }
                        }
                        pending_requests.remove(&id);
                    }.instrument(span));
                }
                TunnelMessage::RequestChunk { id, .. } | TunnelMessage::RequestEnd { id } => {
                    if let Some(request_tx) = pending_requests.get(&id) {
                        let _ = request_tx.value().send(tunnel_msg).map_err(|_| error!("Failed to send chunk"));
                    }
                }
                TunnelMessage::Error { message } => {
                    error!("Server error: {}", message);
                    return Err(anyhow::anyhow!(message));
                }
                _ => warn!("Received unexpected message: {:?}", tunnel_msg),
            }
        }
    }

    Ok(())
}
