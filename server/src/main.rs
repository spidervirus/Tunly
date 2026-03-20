use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use dashmap::DashMap;
use futures_util::{StreamExt, SinkExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn, info_span, Instrument};
use uuid::Uuid;

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

struct Tunnel {
    tx: mpsc::UnboundedSender<TunnelMessage>,
}

type SharedState = Arc<AppState>;

type PendingRequestTx = mpsc::UnboundedSender<TunnelMessage>;

struct AppState {
    tunnels: DashMap<String, Tunnel>,
    pending_requests: DashMap<Uuid, PendingRequestTx>,
    token: String,
    domain: String,
}

async fn run_server(_state: SharedState, app: Router) {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3010".to_string());
    let addr = format!("0.0.0.0:{}", port);
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            info!("Tunly server listening on {}", addr);
            if let Err(e) = axum::serve(listener, app).await {
                error!("Server error: {}", e);
            }
        }
        Err(e) => {
            error!("Failed to bind to {}: {}", addr, e);
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let token = std::env::var("TUNLY_TOKEN").unwrap_or_else(|_| "secret".to_string());
    let domain = std::env::var("TUNLY_DOMAIN").unwrap_or_else(|_| "localhost".to_string());

    let state = Arc::new(AppState {
        tunnels: DashMap::new(),
        pending_requests: DashMap::new(),
        token,
        domain,
    });

    let app = Router::new()
        .route("/tunnel", get(ws_handler))
        .fallback(handle_http)
        .with_state(state.clone());

    run_server(state, app).await;
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: SharedState) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<TunnelMessage>();

    // Step 1: Wait for Register message
    let subdomain = match stream.next().await {
        Some(Ok(Message::Text(text))) => {
            match serde_json::from_str::<TunnelMessage>(&text) {
                Ok(TunnelMessage::Register { token, name }) => {
                    if token != state.token {
                        warn!("Authentication failed for tunnel request");
                        let _ = sink.send(Message::Text(serde_json::to_string(&TunnelMessage::Error { message: "Invalid authentication token".into() }).unwrap())).await;
                        return;
                    }
                    let sub = name.unwrap_or_else(|| Uuid::new_v4().to_string()[..8].to_string());
                    if state.tunnels.contains_key(&sub) {
                        warn!("Subdomain already taken: {}", sub);
                        let _ = sink.send(Message::Text(serde_json::to_string(&TunnelMessage::Error { message: format!("Subdomain '{}' is already in use", sub) }).unwrap())).await;
                        return;
                    }
                    sub
                }
                _ => {
                    warn!("Invalid initial message from client: {}", text);
                    return;
                }
            }
        }
        Some(Err(e)) => {
            error!("WebSocket error during handshake: {}", e);
            return;
        }
        _ => return,
    };

    let span = info_span!("tunnel", subdomain = %subdomain);
    async move {
        info!("Tunnel established");
        state.tunnels.insert(subdomain.clone(), Tunnel { tx });

        let _ = sink.send(Message::Text(serde_json::to_string(&TunnelMessage::Registered { subdomain: subdomain.clone() }).unwrap())).await;

        // Task to send messages from channel to WS
        let sink_task = tokio::spawn({
            async move {
                while let Some(msg) = rx.recv().await {
                    let json = serde_json::to_string(&msg).unwrap();
                    if sink.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
                info!("Closing sink task");
            }.instrument(info_span!("sink"))
        });

        // Task to receive messages from WS
        while let Some(result) = stream.next().await {
            match result {
                Ok(Message::Text(text)) => {
                    if let Ok(tunnel_msg) = serde_json::from_str::<TunnelMessage>(&text) {
                        match tunnel_msg {
                            TunnelMessage::ResponseStart { id, .. } | TunnelMessage::ResponseChunk { id, .. } | TunnelMessage::ResponseEnd { id, .. } => {
                                if let Some(tx) = state.pending_requests.get(&id) {
                                    let _ = tx.value().send(tunnel_msg);
                                }
                            }
                            _ => warn!("Unexpected message from tunnel client: {:?}", tunnel_msg),
                        }
                    }
                }
                Ok(_) => (),
                Err(e) => {
                    error!("WebSocket runtime error: {}", e);
                    break;
                }
            }
        }

        info!("Tunnel disconnected");
        state.tunnels.remove(&subdomain);
        sink_task.abort();
    }.instrument(span).await;
}

async fn handle_http(
    State(state): State<SharedState>,
    req: Request<axum::body::Body>,
) -> Response {
    let host = req.headers().get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Extract subdomain: <subdomain>.<domain>
    let subdomain = if host == state.domain || host.is_empty() {
        None
    } else if host.ends_with(&format!(".{}", state.domain)) {
        Some(host.trim_end_matches(&format!(".{}", state.domain)).to_string())
    } else {
        host.split('.').next().map(|s| s.to_string())
    };

    let subdomain = match subdomain {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "Domain mapping not found").into_response(),
    };

    let span = info_span!("request", subdomain = %subdomain, method = %req.method(), path = %req.uri().path());
    async move {
        let tunnel = match state.tunnels.get(&subdomain) {
            Some(t) => t,
            None => {
                warn!("No active tunnel for subdomain");
                return (StatusCode::NOT_FOUND, format!("Tunnel '{}' not found or offline", subdomain)).into_response();
            }
        };

        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.pending_requests.insert(id, tx);

        let method = req.method().to_string();
        let path = req.uri().path_and_query().map(|pq| pq.to_string()).unwrap_or_else(|| "/".to_string());
        let mut headers = std::collections::HashMap::new();
        for (name, value) in req.headers() {
            headers.insert(name.to_string(), value.to_str().unwrap_or("").to_string());
        }

        let start_msg = TunnelMessage::RequestStart {
            id,
            method,
            path,
            headers,
        };

        if tunnel.tx.send(start_msg).is_err() {
            warn!("Failed to send RequestStart to tunnel client");
            state.pending_requests.remove(&id);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Tunnel connection lost").into_response();
        }

        // Stream request body to client
        let mut body_stream = req.into_body().into_data_stream();
        while let Some(Ok(chunk)) = body_stream.next().await {
            let msg = TunnelMessage::RequestChunk {
                id,
                body: base64::Engine::encode(&base64::prelude::BASE64_STANDARD, chunk),
            };
            if tunnel.tx.send(msg).is_err() {
                break;
            }
        }
        let _ = tunnel.tx.send(TunnelMessage::RequestEnd { id });
        drop(tunnel);

        // Wait for ResponseStart and then stream response back to Axum
        let state_clone = state.clone();
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv()).await {
            Ok(Some(TunnelMessage::ResponseStart { status, headers, .. })) => {
                info!(status = status, "Response started from tunnel");
                let mut res_builder = Response::builder().status(status);
                if let Some(res_headers) = res_builder.headers_mut() {
                    for (k, v) in headers {
                        if let Ok(name) = axum::http::HeaderName::try_from(k) {
                            if let Ok(val) = axum::http::HeaderValue::try_from(v) {
                                res_headers.insert(name, val);
                            }
                        }
                    }
                }
                
                let stream = async_stream::stream! {
                    while let Some(msg) = rx.recv().await {
                        match msg {
                            TunnelMessage::ResponseChunk { body, .. } => {
                                if let Ok(data) = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, body) {
                                    yield Ok::<axum::body::Bytes, core::convert::Infallible>(axum::body::Bytes::from(data));
                                }
                            }
                            TunnelMessage::ResponseEnd { .. } => break,
                            _ => (),
                        }
                    }
                    state_clone.pending_requests.remove(&id);
                };

                res_builder.body(axum::body::Body::from_stream(stream)).unwrap()
            }
            _ => {
                state.pending_requests.remove(&id);
                (StatusCode::GATEWAY_TIMEOUT, "Gateway Timeout: Client did not respond in time").into_response()
            }
        }
    }.instrument(span).await
}
