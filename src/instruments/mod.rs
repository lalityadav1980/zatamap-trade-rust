use crate::{
    core::AppError,
    dao::instrument_dao::{self, InstrumentUpsert},
    kite::client::KiteClient,
};
use chrono::{Duration, Local, NaiveDate};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct KiteInstrumentCsvRow {
    instrument_token: i32,
    #[serde(default)]
    exchange_token: Option<i32>,
    #[serde(default)]
    tradingsymbol: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    expiry: Option<String>,
    #[serde(default)]
    strike: Option<String>,
    #[serde(default)]
    tick_size: Option<String>,
    #[serde(default)]
    lot_size: Option<i32>,
    #[serde(default)]
    instrument_type: Option<String>,
    #[serde(default)]
    segment: Option<String>,
    #[serde(default)]
    exchange: Option<String>,
}

fn clean_opt_string(s: Option<String>) -> Option<String> {
    let s = s?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn parse_strike_to_i64(strike: &Option<String>) -> Option<i64> {
    let s = strike.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    // Python code: int(float(strike))
    let f: f64 = s.parse().ok()?;
    Some(f as i64)
}

fn parse_opt_f64(s: Option<String>) -> Option<f64> {
    let s = s?.trim().to_string();
    if s.is_empty() {
        return None;
    }
    s.parse::<f64>().ok()
}

fn parse_trading_symbol(tradingsymbol: &str, _expiry: Option<&str>) -> String {
    // Matches current Python behavior: just return tradingsymbol.
    tradingsymbol.to_string()
}

fn parse_expiry_date(expiry: Option<&str>) -> Option<NaiveDate> {
    let s = expiry?.trim();
    if s.is_empty() {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Fetch all instruments from Kite and store a filtered subset into Postgres table `trade.instrument`.
///
/// Mirrors Python behavior:
/// - Keep index option chains for NIFTY/BANKNIFTY/FINNIFTY/MIDCPNIFTY/SENSEX (NFO-OPT/BFO-OPT)
/// - Also keep a small set of specific index tokens
/// - Delete existing rows then upsert the new ones
pub async fn refresh_trade_instruments(
    db: &crate::db::Db,
    kite: &KiteClient,
) -> Result<u64, AppError> {
    let started = std::time::Instant::now();

    // Optional optimization: only keep instruments expiring within N days from today.
    // Example: INSTRUMENT_EXPIRY_DAYS=7 keeps "current-week" expiries.
    let expiry_days: Option<i64> = std::env::var("INSTRUMENT_EXPIRY_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0);
    let today = Local::now().date_naive();
    let expiry_end = expiry_days.map(|d| today + Duration::days(d));
    if let Some(d) = expiry_days {
        println!(
            "Instruments: expiry filter enabled days={} window={}..={}",
            d,
            today,
            expiry_end.unwrap()
        );
    }

    println!("Instruments: fetching CSV from Kite...");
    let csv_text = kite.instruments_csv().await?;
    println!(
        "Instruments: downloaded CSV bytes={} elapsed_ms={}",
        csv_text.len(),
        started.elapsed().as_millis()
    );
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(csv_text.as_bytes());

    let mut rows: Vec<KiteInstrumentCsvRow> = Vec::new();
    for rec in rdr.deserialize() {
        let r: KiteInstrumentCsvRow = rec?;
        rows.push(r);
    }
    println!(
        "Instruments: parsed_rows={} elapsed_ms={}",
        rows.len(),
        started.elapsed().as_millis()
    );

    let allowed_names: HashSet<&'static str> = HashSet::from([
        "MIDCPNIFTY",
        "NIFTY",
        "BANKNIFTY",
        "FINNIFTY",
        "SENSEX",
    ]);
    let allowed_segments: HashSet<&'static str> = HashSet::from(["NFO-OPT", "BFO-OPT"]);
    let allowed_exchanges: HashSet<&'static str> = HashSet::from(["NFO", "BFO"]);

    // Same specific tokens as Python.
    let specific_tokens: HashSet<i32> = HashSet::from([256265, 260105, 288009, 257801, 265]);

    let mut selected: Vec<KiteInstrumentCsvRow> = Vec::new();
    let mut selected_specific: usize = 0;
    let mut selected_index_opts: usize = 0;
    for r in rows {
        let is_specific = specific_tokens.contains(&r.instrument_token);

        let name_ok = r
            .name
            .as_deref()
            .map(|n| allowed_names.contains(n))
            .unwrap_or(false);

        let segment_ok = r
            .segment
            .as_deref()
            .map(|s| allowed_segments.contains(s))
            .unwrap_or(false);

        let exchange_ok = r
            .exchange
            .as_deref()
            .map(|e| allowed_exchanges.contains(e))
            .unwrap_or(false);

        let is_index_option = segment_ok && exchange_ok && name_ok;

        let expiry_ok = if let Some(end) = expiry_end {
            let exp = parse_expiry_date(r.expiry.as_deref());
            matches!(exp, Some(d) if d >= today && d <= end)
        } else {
            true
        };

        if is_specific || (is_index_option && expiry_ok) {
            if is_specific {
                selected_specific += 1;
            }
            if is_index_option && expiry_ok {
                selected_index_opts += 1;
            }
            selected.push(r);
        }
    }

    println!(
        "Instruments: selected_total={} selected_specific={} selected_index_opts={} elapsed_ms={}",
        selected.len(),
        selected_specific,
        selected_index_opts,
        started.elapsed().as_millis()
    );

    // Python overwrites name + symbol_full_name for these tokens.
    let name_mapping: HashMap<i32, &'static str> = HashMap::from([
        (256265, "NIFTY"),
        (260105, "BANKNIFTY"),
        (257801, "FINNIFTY"),
        (288009, "MIDCPNIFTY"),
        (265, "SENSEX"),
    ]);

    let mut upserts: Vec<InstrumentUpsert> = Vec::with_capacity(selected.len());
    for r in selected {
        let mut tradingsymbol = clean_opt_string(r.tradingsymbol);
        let mut symbol = clean_opt_string(r.symbol);
        let mut name = clean_opt_string(r.name);
        let last_price = parse_opt_f64(r.last_price);
        let expiry = clean_opt_string(r.expiry);
        let tick_size = parse_opt_f64(r.tick_size);

        // Default symbol_full_name
        let symbol_full_name = match tradingsymbol.as_deref() {
            Some(ts) => Some(parse_trading_symbol(ts, expiry.as_deref())),
            None => None,
        };

        // If this is one of the mapped index tokens, override (matches Python).
        let symbol_full_name = if let Some(mapped) = name_mapping.get(&r.instrument_token) {
            // Keep `name` normalized to the mapping as well.
            name = Some(mapped.to_string());
            Some(mapped.to_string())
        } else {
            symbol_full_name
        };

        // Strike conversion matches Python: int(float(strike))
        let strike = parse_strike_to_i64(&r.strike);

        // The CSV may have empty values that would fail casts; normalize them.
        if let Some(ts) = tradingsymbol.as_ref() {
            if ts.trim().is_empty() {
                tradingsymbol = None;
            }
        }
        if let Some(sym) = symbol.as_ref() {
            if sym.trim().is_empty() {
                symbol = None;
            }
        }

        upserts.push(InstrumentUpsert {
            instrument_token: r.instrument_token,
            exchange_token: r.exchange_token,
            tradingsymbol,
            symbol,
            name,
            last_price,
            expiry,
            strike,
            tick_size,
            lot_size: r.lot_size,
            instrument_type: clean_opt_string(r.instrument_type),
            segment: clean_opt_string(r.segment),
            exchange: clean_opt_string(r.exchange),
            symbol_full_name,
        });
    }

    println!(
        "Instruments: writing to Postgres (delete + upsert) rows={} elapsed_ms={}",
        upserts.len(),
        started.elapsed().as_millis()
    );

    let use_bulk_copy = std::env::var("INSTRUMENT_BULK_COPY")
        .ok()
        .map(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // If we're filtering by expiry window, only refresh the selected tokens (don't wipe the whole table).
    let delete_all = expiry_end.is_none();
    if use_bulk_copy {
        println!(
            "Instruments: using bulk COPY (delete_all={})",
            delete_all
        );
    }

    // Optional optimization: if every selected instrument_token already exists in DB, skip delete/upsert.
    // This avoids a slow remote upsert when data is already present.
    let skip_if_present = std::env::var("INSTRUMENT_SKIP_IF_PRESENT")
        .ok()
        .map(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if skip_if_present {
        let tokens: Vec<i32> = upserts.iter().map(|u| u.instrument_token).collect();
        if !tokens.is_empty() {
            let existing = instrument_dao::count_existing_instrument_tokens(db, &tokens).await?;
            if existing as usize == tokens.len() {
                println!(
                    "Instruments: skip DB write (all tokens already present) tokens={} elapsed_ms={}",
                    tokens.len(),
                    started.elapsed().as_millis()
                );
                return Ok(tokens.len() as u64);
            }
            println!(
                "Instruments: DB has {}/{} tokens; continuing with upsert",
                existing,
                tokens.len()
            );
        }
    }

    let n = if use_bulk_copy {
        instrument_dao::replace_instruments_copy(db, &upserts, delete_all).await?
    } else if delete_all {
        instrument_dao::replace_all_instruments(db, &upserts).await?
    } else {
        instrument_dao::replace_instruments_by_tokens(db, &upserts).await?
    };
    println!(
        "Instruments: done upserted_rows={} total_elapsed_ms={}",
        n,
        started.elapsed().as_millis()
    );
    Ok(n)
}
