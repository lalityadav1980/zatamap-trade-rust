use dashmap::DashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Tick mode (what the server sent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickMode {
    Ltp,
    Quote,
    Full,
}

#[derive(Debug, Clone, Copy)]
pub struct Ohlc {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DepthLevel {
    pub quantity: u32,
    pub price: f64,
    pub orders: u16,
}

#[derive(Debug, Clone)]
pub struct MarketDepth {
    pub buy: [DepthLevel; 5],
    pub sell: [DepthLevel; 5],
}

/// Static metadata for a subscribed instrument.
///
/// This is loaded once from Postgres and stays constant.
#[derive(Debug, Clone)]
pub struct TokenMeta {
    pub instrument_token: i32,
    pub tradingsymbol: Arc<str>,
    pub instrument_type: Arc<str>,
    pub expiry: Option<Arc<str>>, // yyyy-mm-dd (if option/future)
    pub strike: Option<f64>,
}

impl TokenMeta {
    pub fn new(
        instrument_token: i32,
        tradingsymbol: impl Into<Arc<str>>,
        instrument_type: impl Into<Arc<str>>,
        expiry: Option<impl Into<Arc<str>>>,
        strike: Option<f64>,
    ) -> Self {
        Self {
            instrument_token,
            tradingsymbol: tradingsymbol.into(),
            instrument_type: instrument_type.into(),
            expiry: expiry.map(|v| v.into()),
            strike,
        }
    }
}

/// Derived metrics we compute incrementally in the tick store.
///
/// These are stored per-token so downstream strategies can read without
/// re-computing on every access.
#[derive(Debug, Clone, Default)]
pub struct DerivedMetrics {
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub spread: Option<f64>,
    pub spread_bps: Option<f64>,

    pub price_roc_per_s: Option<f64>,
    pub oi_roc_per_s: Option<f64>,
    pub vol_roc_per_s: Option<f64>,
}

/// Normalized tick representation used across the Rust codebase.
///
/// Notes:
/// - Prices are converted into rupees (Kite sends paise as integers).
/// - Timestamps (when present) are UNIX seconds (as sent by Kite).
#[derive(Debug, Clone)]
pub struct Tick {
    pub instrument_token: i32,
    pub mode: TickMode,

    pub last_price: f64,

    // Quote/full fields
    pub last_quantity: Option<u32>,
    pub average_traded_price: Option<f64>,
    pub volume_traded: Option<u32>,
    pub total_buy_quantity: Option<u32>,
    pub total_sell_quantity: Option<u32>,
    pub ohlc: Option<Ohlc>,
    pub change: Option<f64>,

    // Full-only fields
    pub last_trade_time: Option<u32>,
    pub open_interest: Option<u32>,
    pub oi_day_high: Option<u32>,
    pub oi_day_low: Option<u32>,
    pub exchange_timestamp: Option<u32>,
    pub depth: Option<MarketDepth>,

    // When this process received the tick (UNIX ns)
    pub received_ns: u64,
}

impl Tick {
    pub fn new_ltp(instrument_token: i32, last_price: f64, received_ns: u64) -> Self {
        Self {
            instrument_token,
            mode: TickMode::Ltp,
            last_price,
            last_quantity: None,
            average_traded_price: None,
            volume_traded: None,
            total_buy_quantity: None,
            total_sell_quantity: None,
            ohlc: None,
            change: None,
            last_trade_time: None,
            open_interest: None,
            oi_day_high: None,
            oi_day_low: None,
            exchange_timestamp: None,
            depth: None,
            received_ns,
        }
    }
}

/// Per-token state kept in memory.
///
/// This is the primary shared structure other modules read from.
#[derive(Debug, Clone)]
pub struct TokenState {
    pub meta: TokenMeta,
    pub last_tick: Option<Tick>,
    pub derived: DerivedMetrics,
}

impl TokenState {
    pub fn new(meta: TokenMeta) -> Self {
        Self {
            meta,
            last_tick: None,
            derived: DerivedMetrics::default(),
        }
    }
}

/// Shared in-memory store for the latest tick per token.
///
/// This is designed to be read frequently by other modules (signals/strategy)
/// while a single websocket task keeps updating it.
#[derive(Debug, Default)]
pub struct TickStore {
    by_token: DashMap<i32, TokenState>,
}

impl TickStore {
    /// Seed metadata for subscribed instruments.
    ///
    /// Call this once before websocket starts so the store has the
    /// token→tradingsymbol map ready for downstream consumers.
    pub fn seed_meta(&self, metas: impl IntoIterator<Item = TokenMeta>) {
        for meta in metas {
            self.by_token
                .entry(meta.instrument_token)
                .or_insert_with(|| TokenState::new(meta));
        }
    }

    /// Update a token state with the latest tick.
    ///
    /// This updates derived metrics (spread + ROC) incrementally.
    pub fn update_tick(&self, tick: Tick) {
        let token = tick.instrument_token;
        if let Some(mut state) = self.by_token.get_mut(&token) {
            // ROC calculations require previous values.
            if let Some(prev) = state.last_tick.as_ref() {
                let prev_received_ns = prev.received_ns;
                let prev_last_price = prev.last_price;
                let prev_oi = prev.open_interest;
                let prev_vol = prev.volume_traded;

                let dt_ns = tick.received_ns.saturating_sub(prev_received_ns);
                let dt_s = (dt_ns as f64) / 1_000_000_000.0;
                if dt_s > 0.0 {
                    state.derived.price_roc_per_s = Some((tick.last_price - prev_last_price) / dt_s);

                    match (tick.open_interest, prev_oi) {
                        (Some(oi), Some(poi)) => {
                            state.derived.oi_roc_per_s = Some(((oi as f64) - (poi as f64)) / dt_s)
                        }
                        _ => {}
                    }

                    match (tick.volume_traded, prev_vol) {
                        (Some(v), Some(pv)) => {
                            state.derived.vol_roc_per_s = Some(((v as f64) - (pv as f64)) / dt_s)
                        }
                        _ => {}
                    }
                }
            }

            // Spread from depth (FULL mode).
            if let Some(depth) = tick.depth.as_ref() {
                let bid = depth.buy[0].price;
                let ask = depth.sell[0].price;
                state.derived.best_bid = Some(bid);
                state.derived.best_ask = Some(ask);
                let spread = ask - bid;
                state.derived.spread = Some(spread);
                state.derived.spread_bps = if tick.last_price > 0.0 {
                    Some((spread / tick.last_price) * 10_000.0)
                } else {
                    None
                };
            }

            state.last_tick = Some(tick);
            return;
        }

        // Fallback: unknown token. Insert minimal meta so we still store the tick.
        let meta = TokenMeta::new(token, "", "UNKNOWN", Option::<Arc<str>>::None, None);
        let mut state = TokenState::new(meta);
        state.last_tick = Some(tick);
        self.by_token.insert(token, state);
    }

    pub fn get_state(&self, instrument_token: i32) -> Option<TokenState> {
        self.by_token.get(&instrument_token).map(|v| v.clone())
    }

    pub fn get_symbol(&self, instrument_token: i32) -> Option<Arc<str>> {
        self.by_token
            .get(&instrument_token)
            .map(|s| s.meta.tradingsymbol.clone())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn len(&self) -> usize {
        self.by_token.len()
    }

    /// Number of tokens for which we have received at least one tick.
    pub fn received_token_count(&self) -> usize {
        self.by_token
            .iter()
            .filter(|kv| kv.value().last_tick.is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.by_token.is_empty()
    }
}

/// Decode Kite's binary ticker payload into a list of ticks.
///
/// Kite packs multiple tick "packets" in a single binary frame:
/// - First 2 bytes: number of packets (u16, big-endian)
/// - For each packet:
///   - 2 bytes: packet length (u16, big-endian)
///   - N bytes: packet payload
pub fn decode_binary_ticks(payload: &[u8], received_ns: u64) -> Vec<Tick> {
    let mut out = Vec::new();
    if payload.len() < 2 {
        return out;
    }

    let mut offset = 0usize;
    let n_packets = read_u16_be(payload, &mut offset).unwrap_or(0) as usize;

    for _ in 0..n_packets {
        let Some(packet_len) = read_u16_be(payload, &mut offset) else {
            break;
        };
        let packet_len = packet_len as usize;
        if offset + packet_len > payload.len() {
            break;
        }
        let packet = &payload[offset..offset + packet_len];
        offset += packet_len;

        if let Some(tick) = decode_packet(packet, received_ns) {
            out.push(tick);
        }
    }

    out
}

fn decode_packet(packet: &[u8], received_ns: u64) -> Option<Tick> {
    // All known packet types start with 4-byte instrument token.
    if packet.len() < 8 {
        return None;
    }

    let mut offset = 0usize;
    let token_u32 = read_u32_be(packet, &mut offset)?;
    let instrument_token = token_u32 as i32;

    // Next always contains last_price (i32 paise).
    let last_price_i32 = read_i32_be(packet, &mut offset)?;
    let last_price = (last_price_i32 as f64) / 100.0;

    // Packet type is inferred from length (Kite convention).
    match packet.len() {
        8 => Some(Tick::new_ltp(instrument_token, last_price, received_ns)),

        // Index packet (commonly 28 bytes): token + (6 * i32 fields)
        // last_price, high, low, open, close, change
        28 => {
            let high = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let low = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let open = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let close = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let change = (read_i32_be(packet, &mut offset)? as f64) / 100.0;

            Some(Tick {
                instrument_token,
                mode: TickMode::Quote,
                last_price,
                last_quantity: None,
                average_traded_price: None,
                volume_traded: None,
                total_buy_quantity: None,
                total_sell_quantity: None,
                ohlc: Some(Ohlc {
                    open,
                    high,
                    low,
                    close,
                }),
                change: Some(change),
                last_trade_time: None,
                open_interest: None,
                oi_day_high: None,
                oi_day_low: None,
                exchange_timestamp: None,
                depth: None,
                received_ns,
            })
        }

        // Quote packet (commonly 44 bytes)
        44 => {
            let last_quantity = read_u32_be(packet, &mut offset)?;
            let avg = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let volume = read_u32_be(packet, &mut offset)?;
            let buy_qty = read_u32_be(packet, &mut offset)?;
            let sell_qty = read_u32_be(packet, &mut offset)?;
            let open = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let high = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let low = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let close = (read_i32_be(packet, &mut offset)? as f64) / 100.0;

            Some(Tick {
                instrument_token,
                mode: TickMode::Quote,
                last_price,
                last_quantity: Some(last_quantity),
                average_traded_price: Some(avg),
                volume_traded: Some(volume),
                total_buy_quantity: Some(buy_qty),
                total_sell_quantity: Some(sell_qty),
                ohlc: Some(Ohlc {
                    open,
                    high,
                    low,
                    close,
                }),
                change: Some(if close != 0.0 {
                    (last_price - close) / close
                } else {
                    0.0
                }),
                last_trade_time: None,
                open_interest: None,
                oi_day_high: None,
                oi_day_low: None,
                exchange_timestamp: None,
                depth: None,
                received_ns,
            })
        }

        // Full packet (commonly 184 bytes): quote + timestamps + OI + depth(10 levels).
        184 => {
            let last_quantity = read_u32_be(packet, &mut offset)?;
            let avg = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let volume = read_u32_be(packet, &mut offset)?;
            let buy_qty = read_u32_be(packet, &mut offset)?;
            let sell_qty = read_u32_be(packet, &mut offset)?;
            let open = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let high = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let low = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
            let close = (read_i32_be(packet, &mut offset)? as f64) / 100.0;

            let last_trade_time = read_u32_be(packet, &mut offset)?;
            let oi = read_u32_be(packet, &mut offset)?;
            let oi_day_high = read_u32_be(packet, &mut offset)?;
            let oi_day_low = read_u32_be(packet, &mut offset)?;
            let exchange_timestamp = read_u32_be(packet, &mut offset)?;

            // Depth: 10 levels, 5 buy + 5 sell.
            let mut buy = [DepthLevel {
                quantity: 0,
                price: 0.0,
                orders: 0,
            }; 5];
            let mut sell = [DepthLevel {
                quantity: 0,
                price: 0.0,
                orders: 0,
            }; 5];

            // Each level is 12 bytes: quantity(u32) + price(i32 paise) + orders(u16) + reserved(u16)
            for i in 0..5 {
                let q = read_u32_be(packet, &mut offset)?;
                let p = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
                let orders = read_u16_be(packet, &mut offset)?;
                let _reserved = read_u16_be(packet, &mut offset)?;
                buy[i] = DepthLevel {
                    quantity: q,
                    price: p,
                    orders,
                };
            }
            for i in 0..5 {
                let q = read_u32_be(packet, &mut offset)?;
                let p = (read_i32_be(packet, &mut offset)? as f64) / 100.0;
                let orders = read_u16_be(packet, &mut offset)?;
                let _reserved = read_u16_be(packet, &mut offset)?;
                sell[i] = DepthLevel {
                    quantity: q,
                    price: p,
                    orders,
                };
            }

            Some(Tick {
                instrument_token,
                mode: TickMode::Full,
                last_price,
                last_quantity: Some(last_quantity),
                average_traded_price: Some(avg),
                volume_traded: Some(volume),
                total_buy_quantity: Some(buy_qty),
                total_sell_quantity: Some(sell_qty),
                ohlc: Some(Ohlc {
                    open,
                    high,
                    low,
                    close,
                }),
                change: Some(if close != 0.0 {
                    (last_price - close) / close
                } else {
                    0.0
                }),
                last_trade_time: Some(last_trade_time),
                open_interest: Some(oi),
                oi_day_high: Some(oi_day_high),
                oi_day_low: Some(oi_day_low),
                exchange_timestamp: Some(exchange_timestamp),
                depth: Some(MarketDepth { buy, sell }),
                received_ns,
            })
        }

        // Unknown packet size: ignore safely.
        _ => None,
    }
}

/// Fast timestamp helper for the websocket hot path.
pub fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn read_u16_be(buf: &[u8], offset: &mut usize) -> Option<u16> {
    if *offset + 2 > buf.len() {
        return None;
    }
    let v = u16::from_be_bytes([buf[*offset], buf[*offset + 1]]);
    *offset += 2;
    Some(v)
}

fn read_u32_be(buf: &[u8], offset: &mut usize) -> Option<u32> {
    if *offset + 4 > buf.len() {
        return None;
    }
    let v = u32::from_be_bytes([
        buf[*offset],
        buf[*offset + 1],
        buf[*offset + 2],
        buf[*offset + 3],
    ]);
    *offset += 4;
    Some(v)
}

fn read_i32_be(buf: &[u8], offset: &mut usize) -> Option<i32> {
    read_u32_be(buf, offset).map(|v| v as i32)
}
