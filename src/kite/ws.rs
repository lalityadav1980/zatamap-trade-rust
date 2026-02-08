use crate::core::AppError;
use crate::ticks::{decode_binary_ticks, now_unix_ns, TickStore};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::tungstenite::http::header::HeaderValue;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

fn env_bool_default(key: &str, default: bool) -> bool {
    let Some(v) = std::env::var(key).ok() else {
        return default;
    };

    let v = v.trim();
    if v.is_empty() {
        return default;
    }

    match v {
        "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON" => true,
        "0" | "false" | "FALSE" | "no" | "NO" | "off" | "OFF" => false,
        _ => default,
    }
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|v| v.trim().parse::<u64>().ok())
}

#[derive(Clone, Debug)]
pub struct TickLogConfig {
    pub enabled: bool,
    pub interval: Duration,
}

impl TickLogConfig {
    /// Default: enabled=true, interval=500ms.
    ///
    /// Env:
    /// - TICK_LOG_FULL (default 1/on; set 0/off to disable)
    /// - TICK_LOG_INTERVAL_MS (default 500)
    pub fn from_env() -> Self {
        let enabled = env_bool_default("TICK_LOG_FULL", true);
        let interval_ms = env_u64("TICK_LOG_INTERVAL_MS")
            .filter(|v| *v > 0)
            .unwrap_or(500);
        Self {
            enabled,
            interval: Duration::from_millis(interval_ms),
        }
    }
}

/// Zerodha Kite ticker websocket client.
///
/// Responsibilities:
/// - Connect to `wss://ws.kite.trade` using api_key + access_token
/// - Subscribe to tokens and set mode=full
/// - Decode incoming binary tick frames
/// - Upsert latest tick per token into `TickStore`
/// - Reconnect with backoff on disconnect/error
#[derive(Clone)]
pub struct KiteTickerWs {
    api_key: String,
    access_token: String,
    tokens: Arc<Vec<i32>>,
    allowed: Arc<HashSet<i32>>,
    store: Arc<TickStore>,
    log: TickLogConfig,
}

impl KiteTickerWs {
    pub fn new(
        api_key: String,
        access_token: String,
        tokens: Vec<i32>,
        store: Arc<TickStore>,
        log: TickLogConfig,
    ) -> Self {
        let allowed: HashSet<i32> = tokens.iter().copied().collect();
        Self {
            api_key,
            access_token,
            tokens: Arc::new(tokens),
            allowed: Arc::new(allowed),
            store,
            log,
        }
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run_forever().await {
                warn!(error = %e, "kite ticker ws exited");
            }
        })
    }

    async fn run_forever(&self) -> Result<(), AppError> {
        // Exponential backoff reconnect strategy.
        let mut backoff = Duration::from_millis(250);
        let max_backoff = Duration::from_secs(30);

        loop {
            match self.run_once().await {
                Ok(()) => {
                    // A clean close still reconnects (server can drop idle connections).
                    backoff = Duration::from_millis(250);
                }
                Err(e) => {
                    warn!(error = %e, sleep_ms = backoff.as_millis() as u64, "kite ws error; reconnecting");
                    sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                }
            }
        }
    }

    async fn run_once(&self) -> Result<(), AppError> {
        if self.tokens.is_empty() {
            return Err(AppError::KiteApi("no tokens to subscribe".to_string()));
        }

        let url = format!(
            "wss://ws.kite.trade/?api_key={}&access_token={}",
            urlencoding::encode(&self.api_key),
            urlencoding::encode(&self.access_token)
        );

        info!(token_count = self.tokens.len(), "connecting kite ticker websocket");
        let mut req = url
            .into_client_request()
            .map_err(|e| AppError::KiteApi(format!("ws request build failed: {e}")))?;

        // Kite's WS endpoint expects a browser-like handshake.
        // Without Origin it often rejects with HTTP 400.
        req.headers_mut().insert(
            "Origin",
            HeaderValue::from_static("https://kite.zerodha.com"),
        );
        req.headers_mut().insert(
            "User-Agent",
            HeaderValue::from_static("zatamap-trade-rust/0.1"),
        );
        req.headers_mut().insert(
            "X-Kite-Version",
            HeaderValue::from_static("3"),
        );

        let (ws_stream, resp) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(|e| AppError::KiteApi(format!("ws connect failed: {e}")))?;

        info!(status = %resp.status(), "kite ws connected");

        let (mut write, mut read) = ws_stream.split();

        self.subscribe_full(&mut write).await?;
        info!(token_count = self.tokens.len(), "subscribed + mode=full");

        let log_full_ticks = self.log.enabled;
        let log_interval = self.log.interval;
        let mut last_tick_log = std::time::Instant::now();
        let mut logged_first_per_token: HashSet<i32> = HashSet::new();

        // Read loop: decode binary ticks; log server messages.
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Binary(bin)) => {
                    let received_ns = now_unix_ns();
                    let ticks = decode_binary_ticks(&bin, received_ns);
                    if !ticks.is_empty() {
                        for t in ticks {
                            // Defensive: keep memory bounded even if server sends
                            // unexpected tokens.
                            if self.allowed.contains(&t.instrument_token) {
                                if log_full_ticks {
                                    let first_for_token = logged_first_per_token.insert(t.instrument_token);
                                    let due = last_tick_log.elapsed() >= log_interval;
                                    if first_for_token || due {
                                        let symbol = self
                                            .store
                                            .get_symbol(t.instrument_token)
                                            .unwrap_or_else(|| Arc::<str>::from(""));
                                        info!(
                                            instrument_token = t.instrument_token,
                                            tradingsymbol = %symbol,
                                            tick = ?t,
                                            "kite tick"
                                        );
                                        last_tick_log = std::time::Instant::now();
                                    }
                                }
                                self.store.update_tick(t);
                            }
                        }
                    }
                }
                Ok(Message::Text(txt)) => {
                    // Kite sends JSON control frames (error/order/connection).
                    debug!(message = %txt, "kite ws text");
                }
                Ok(Message::Ping(p)) => {
                    // tungstenite will auto-handle ping/pong in many cases, but we can be explicit.
                    write
                        .send(Message::Pong(p))
                        .await
                        .map_err(|e| AppError::KiteApi(format!("ws pong send failed: {e}")))?;
                }
                Ok(Message::Pong(_)) => {}
                Ok(Message::Close(frame)) => {
                    info!(close = ?frame, "kite ws close");
                    return Ok(());
                }
                Err(e) => {
                    return Err(AppError::KiteApi(format!("ws read error: {e}")));
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn subscribe_full(
        &self,
        write: &mut (impl SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    ) -> Result<(), AppError> {
        // Subscribe in chunks to keep message sizes reasonable.
        const CHUNK: usize = 300;
        for chunk in self.tokens.chunks(CHUNK) {
            let msg = json!({"a":"subscribe","v":chunk});
            write
                .send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| AppError::KiteApi(format!("ws subscribe send failed: {e}")))?;

            let mode_msg = json!({"a":"mode","v":["full", chunk]});
            write
                .send(Message::Text(mode_msg.to_string()))
                .await
                .map_err(|e| AppError::KiteApi(format!("ws mode send failed: {e}")))?;
        }
        Ok(())
    }
}
