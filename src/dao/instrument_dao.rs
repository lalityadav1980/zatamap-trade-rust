use crate::{core::AppError, db::Db};
use bytes::Bytes;
use futures_util::SinkExt;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct InstrumentUpsert {
    pub instrument_token: i32,
    pub exchange_token: Option<i32>,
    pub tradingsymbol: Option<String>,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub last_price: Option<f64>,
    pub expiry: Option<String>,
    pub strike: Option<i64>,
    pub tick_size: Option<f64>,
    pub lot_size: Option<i32>,
    pub instrument_type: Option<String>,
    pub segment: Option<String>,
    pub exchange: Option<String>,
    pub symbol_full_name: Option<String>,
}

pub async fn count_existing_instrument_tokens(db: &Db, tokens: &[i32]) -> Result<i64, AppError> {
    if tokens.is_empty() {
        return Ok(0);
    }
    let client = db.client();
    let row = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM trade.instrument WHERE instrument_token = ANY($1)",
            &[&tokens],
        )
        .await?;
    let n: i64 = row.get(0);
    Ok(n)
}

pub async fn replace_all_instruments(db: &Db, instruments: &[InstrumentUpsert]) -> Result<u64, AppError> {
    let client = db.client();
    let started = std::time::Instant::now();
    info!(rows = instruments.len(), "instrument replace_all begin");
    client.batch_execute("BEGIN").await?;

    let r: Result<u64, AppError> = async {
        let deleted = client.execute("DELETE FROM trade.instrument", &[]).await?;
        info!(deleted_rows = deleted, elapsed_ms = started.elapsed().as_millis() as u64, "instrument delete_all done");

                let stmt = client
                        .prepare(
                                r#"
INSERT INTO trade.instrument (
    instrument_token,
    exchange_token,
    tradingsymbol,
    symbol,
    name,
    last_price,
    expiry,
    strike,
    tick_size,
    lot_size,
    instrument_type,
    segment,
    exchange,
    fetched_at,
    symbol_full_name
) VALUES (
    $1,
    $2,
    $3,
    $4,
    $5,
    $6::float8,
    $7::text::date,
    $8::int8,
    $9::float8,
    $10,
    $11,
    $12,
    $13,
    NOW(),
    $14
)
ON CONFLICT (instrument_token) DO UPDATE SET
    exchange_token   = EXCLUDED.exchange_token,
    tradingsymbol    = EXCLUDED.tradingsymbol,
    symbol           = EXCLUDED.symbol,
    name             = EXCLUDED.name,
    last_price       = EXCLUDED.last_price,
    expiry           = EXCLUDED.expiry,
    strike           = EXCLUDED.strike,
    tick_size        = EXCLUDED.tick_size,
    lot_size         = EXCLUDED.lot_size,
    instrument_type  = EXCLUDED.instrument_type,
    segment          = EXCLUDED.segment,
    exchange         = EXCLUDED.exchange,
    fetched_at       = NOW(),
    symbol_full_name = EXCLUDED.symbol_full_name
"#,
                        )
                        .await?;

        let mut n: u64 = 0;
        for i in instruments {
            let expiry = i.expiry.as_deref();
            let symbol_full_name = i.symbol_full_name.as_deref();
            let tradingsymbol = i.tradingsymbol.as_deref();
            let symbol = i.symbol.as_deref();
            let name = i.name.as_deref();

            n += client
                .execute(
                    &stmt,
                    &[
                        &i.instrument_token,
                        &i.exchange_token,
                        &tradingsymbol,
                        &symbol,
                        &name,
                        &i.last_price,
                        &expiry,
                        &i.strike,
                        &i.tick_size,
                        &i.lot_size,
                        &i.instrument_type,
                        &i.segment,
                        &i.exchange,
                        &symbol_full_name,
                    ],
                )
                .await?;

            if n % 5_000 == 0 {
                info!(rows = n, elapsed_ms = started.elapsed().as_millis() as u64, "instrument upsert progress");
            }
        }

        Ok(n)
    }
    .await;

    match r {
        Ok(n) => {
            client.batch_execute("COMMIT").await?;
            info!(upserted_rows = n, total_elapsed_ms = started.elapsed().as_millis() as u64, "instrument replace_all commit");
            Ok(n)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            info!(error = %e, total_elapsed_ms = started.elapsed().as_millis() as u64, "instrument replace_all rollback");
            Err(e)
        }
    }
}

fn copy_escape_text_field(s: &str) -> String {
    // COPY ... FROM STDIN WITH (FORMAT text) escaping rules for special chars.
    // (Tab/newline/backslash must be escaped.)
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

fn copy_field_opt_str(v: Option<&str>) -> String {
    match v {
        None => "\\N".to_string(),
        Some(s) => copy_escape_text_field(s),
    }
}

fn copy_field_opt_i32(v: Option<i32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "\\N".to_string())
}

fn copy_field_opt_i64(v: Option<i64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "\\N".to_string())
}

fn copy_field_opt_f64(v: Option<f64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "\\N".to_string())
}

pub async fn replace_instruments_by_tokens(
    db: &Db,
    instruments: &[InstrumentUpsert],
) -> Result<u64, AppError> {
    if instruments.is_empty() {
        return Ok(0);
    }

    let client = db.client();
    let started = std::time::Instant::now();
    info!(rows = instruments.len(), "instrument replace_by_tokens begin");
    client.batch_execute("BEGIN").await?;

    let r: Result<u64, AppError> = async {
        let tokens: Vec<i32> = instruments.iter().map(|i| i.instrument_token).collect();
        let deleted = client
            .execute(
                "DELETE FROM trade.instrument WHERE instrument_token = ANY($1)",
                &[&tokens],
            )
            .await?;
        info!(deleted_rows = deleted, elapsed_ms = started.elapsed().as_millis() as u64, "instrument delete_by_tokens done");

        let stmt = client
            .prepare(
                r#"
INSERT INTO trade.instrument (
    instrument_token,
    exchange_token,
    tradingsymbol,
    symbol,
    name,
    last_price,
    expiry,
    strike,
    tick_size,
    lot_size,
    instrument_type,
    segment,
    exchange,
    fetched_at,
    symbol_full_name
) VALUES (
    $1,
    $2,
    $3,
    $4,
    $5,
    $6::float8,
    $7::text::date,
    $8::int8,
    $9::float8,
    $10,
    $11,
    $12,
    $13,
    NOW(),
    $14
)
ON CONFLICT (instrument_token) DO UPDATE SET
    exchange_token   = EXCLUDED.exchange_token,
    tradingsymbol    = EXCLUDED.tradingsymbol,
    symbol           = EXCLUDED.symbol,
    name             = EXCLUDED.name,
    last_price       = EXCLUDED.last_price,
    expiry           = EXCLUDED.expiry,
    strike           = EXCLUDED.strike,
    tick_size        = EXCLUDED.tick_size,
    lot_size         = EXCLUDED.lot_size,
    instrument_type  = EXCLUDED.instrument_type,
    segment          = EXCLUDED.segment,
    exchange         = EXCLUDED.exchange,
    fetched_at       = NOW(),
    symbol_full_name = EXCLUDED.symbol_full_name
"#,
            )
            .await?;

        let mut n: u64 = 0;
        for i in instruments {
            let expiry = i.expiry.as_deref();
            let symbol_full_name = i.symbol_full_name.as_deref();
            let tradingsymbol = i.tradingsymbol.as_deref();
            let symbol = i.symbol.as_deref();
            let name = i.name.as_deref();

            n += client
                .execute(
                    &stmt,
                    &[
                        &i.instrument_token,
                        &i.exchange_token,
                        &tradingsymbol,
                        &symbol,
                        &name,
                        &i.last_price,
                        &expiry,
                        &i.strike,
                        &i.tick_size,
                        &i.lot_size,
                        &i.instrument_type,
                        &i.segment,
                        &i.exchange,
                        &symbol_full_name,
                    ],
                )
                .await?;

            if n % 5_000 == 0 {
                info!(rows = n, elapsed_ms = started.elapsed().as_millis() as u64, "instrument upsert progress");
            }
        }
        Ok(n)
    }
    .await;

    match r {
        Ok(n) => {
            client.batch_execute("COMMIT").await?;
            info!(upserted_rows = n, total_elapsed_ms = started.elapsed().as_millis() as u64, "instrument replace_by_tokens commit");
            Ok(n)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            info!(error = %e, total_elapsed_ms = started.elapsed().as_millis() as u64, "instrument replace_by_tokens rollback");
            Err(e)
        }
    }
}

/// Fetch NIFTY weekly option tokens for the *nearest* expiry within the window.
///
/// This is intended for websocket subscriptions where you only want the current
/// weekly series (similar to Python's `get_next_nifty_weekly_series_df()`).
///
/// Selection logic:
/// - Underlying: `name = 'NIFTY'`
/// - Options only: `instrument_type IN ('CE','PE')` and `exchange = 'NFO'`
/// - Expiry window: [today, today + expiry_days]
/// - Then pick the minimum expiry date in that window (single series)
pub async fn fetch_nifty_current_week_option_tokens(
    db: &Db,
    expiry_days: i32,
) -> Result<(Option<String>, Vec<i32>), AppError> {
    let expiry_days = expiry_days.clamp(1, 14);
    let client = db.client();

    let expiry_row = client
        .query_opt(
            r#"
SELECT (MIN(expiry)::date)::text
FROM trade.instrument
WHERE exchange = 'NFO'
  AND instrument_type IN ('CE','PE')
  AND name = 'NIFTY'
  AND expiry >= CURRENT_DATE
  AND expiry <= (CURRENT_DATE + ($1::int * INTERVAL '1 day'))
"#,
            &[&expiry_days],
        )
        .await?;

    let Some(row) = expiry_row else {
        warn!(expiry_days = expiry_days, "no NIFTY options found in expiry window");
        return Ok((None, vec![]));
    };

    let expiry: Option<String> = row.get(0);
    let Some(expiry) = expiry.filter(|s| !s.trim().is_empty()) else {
        warn!(expiry_days = expiry_days, "no NIFTY expiry in window");
        return Ok((None, vec![]));
    };

    let rows = client
        .query(
            r#"
SELECT instrument_token
FROM trade.instrument
WHERE exchange = 'NFO'
  AND instrument_type IN ('CE','PE')
  AND name = 'NIFTY'
    AND expiry = $1::text::date
ORDER BY instrument_token
"#,
            &[&expiry],
        )
        .await?;

    let mut tokens = Vec::with_capacity(rows.len());
    for r in rows {
        let t: i32 = r.get(0);
        tokens.push(t);
    }

    info!(expiry = %expiry, tokens = tokens.len(), "selected current-week NIFTY option tokens");
    Ok((Some(expiry), tokens))
}

/// Minimal metadata required to build a token→tradingsymbol map and option math inputs.
#[derive(Debug, Clone)]
pub struct InstrumentMetaRow {
    pub instrument_token: i32,
    pub tradingsymbol: String,
    pub instrument_type: String,
    pub expiry: Option<String>,
    pub strike: Option<f64>,
}

/// Fetch NIFTY weekly option *metadata* for the nearest expiry within the window.
///
/// This returns enough fields to:
/// - map token → tradingsymbol (for logging + downstream correlation)
/// - keep strike/expiry for future greek calculations
pub async fn fetch_nifty_current_week_option_meta(
    db: &Db,
    expiry_days: i32,
) -> Result<(Option<String>, Vec<InstrumentMetaRow>), AppError> {
    let (expiry, _tokens) = fetch_nifty_current_week_option_tokens(db, expiry_days).await?;
    let Some(expiry) = expiry else {
        return Ok((None, vec![]));
    };

    let client = db.client();
    let rows = client
        .query(
            r#"
SELECT instrument_token,
       COALESCE(tradingsymbol, '') as tradingsymbol,
       COALESCE(instrument_type, '') as instrument_type,
       expiry::text as expiry,
             strike::float8 as strike
FROM trade.instrument
WHERE exchange = 'NFO'
  AND instrument_type IN ('CE','PE')
  AND name = 'NIFTY'
  AND expiry = $1::text::date
ORDER BY instrument_token
"#,
            &[&expiry],
        )
        .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(InstrumentMetaRow {
            instrument_token: r.get::<_, i32>(0),
            tradingsymbol: r.get::<_, String>(1),
            instrument_type: r.get::<_, String>(2),
            expiry: r.get::<_, Option<String>>(3),
            strike: r.get::<_, Option<f64>>(4),
        });
    }

    info!(expiry = %expiry, rows = out.len(), "selected current-week NIFTY option meta");
    Ok((Some(expiry), out))
}

pub async fn replace_instruments_copy(
    db: &Db,
    instruments: &[InstrumentUpsert],
    delete_all: bool,
) -> Result<u64, AppError> {
    if instruments.is_empty() {
        return Ok(0);
    }

    let client = db.client();
    let started = std::time::Instant::now();
    info!(rows = instruments.len(), delete_all = delete_all, "instrument copy begin");
    client.batch_execute("BEGIN").await?;

    let r: Result<u64, AppError> = async {
        if delete_all {
            let deleted = client.execute("DELETE FROM trade.instrument", &[]).await?;
            info!(deleted_rows = deleted, elapsed_ms = started.elapsed().as_millis() as u64, "instrument delete_all done");
        } else {
            let tokens: Vec<i32> = instruments.iter().map(|i| i.instrument_token).collect();
            let deleted = client
                .execute(
                    "DELETE FROM trade.instrument WHERE instrument_token = ANY($1)",
                    &[&tokens],
                )
                .await?;
            info!(deleted_rows = deleted, elapsed_ms = started.elapsed().as_millis() as u64, "instrument delete_by_tokens done");
        }

        // Stage into a temp table (all TEXT) so COPY stays simple and the final insert is set-based.
        client
            .batch_execute(
                r#"
CREATE TEMP TABLE tmp_instruments (
  instrument_token  text,
  exchange_token    text,
  tradingsymbol     text,
  symbol            text,
  name              text,
  last_price        text,
  expiry            text,
  strike            text,
  tick_size         text,
  lot_size          text,
  instrument_type   text,
  segment           text,
  exchange          text,
  symbol_full_name  text
) ON COMMIT DROP
"#,
            )
            .await?;

        let copy_stmt = "COPY tmp_instruments (instrument_token, exchange_token, tradingsymbol, symbol, name, last_price, expiry, strike, tick_size, lot_size, instrument_type, segment, exchange, symbol_full_name) FROM STDIN";
        let sink = client.copy_in(copy_stmt).await?;
        let mut sink = std::pin::pin!(sink);

        // Stream in chunks to avoid one huge allocation.
        let mut buf = String::with_capacity(256 * 1024);
        let mut sent_rows: u64 = 0;
        for i in instruments {
            let line = format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                i.instrument_token,
                copy_field_opt_i32(i.exchange_token),
                copy_field_opt_str(i.tradingsymbol.as_deref()),
                copy_field_opt_str(i.symbol.as_deref()),
                copy_field_opt_str(i.name.as_deref()),
                copy_field_opt_f64(i.last_price),
                copy_field_opt_str(i.expiry.as_deref()),
                copy_field_opt_i64(i.strike),
                copy_field_opt_f64(i.tick_size),
                copy_field_opt_i32(i.lot_size),
                copy_field_opt_str(i.instrument_type.as_deref()),
                copy_field_opt_str(i.segment.as_deref()),
                copy_field_opt_str(i.exchange.as_deref()),
                copy_field_opt_str(i.symbol_full_name.as_deref()),
            );
            buf.push_str(&line);
            sent_rows += 1;

            if buf.len() >= 256 * 1024 {
                let chunk = std::mem::take(&mut buf);
                sink.as_mut().send(Bytes::from(chunk)).await?;
            }

            if sent_rows % 25_000 == 0 {
                info!(rows = sent_rows, elapsed_ms = started.elapsed().as_millis() as u64, "instrument copy progress");
            }
        }

        if !buf.is_empty() {
            sink.as_mut().send(Bytes::from(buf)).await?;
        }

        let copied_rows = sink.as_mut().finish().await?;
        info!(rows = copied_rows, elapsed_ms = started.elapsed().as_millis() as u64, "instrument copy done");

        // Set-based insert. (Delete already handled, so no ON CONFLICT needed.)
        let inserted = client
            .execute(
                r#"
INSERT INTO trade.instrument (
  instrument_token,
  exchange_token,
  tradingsymbol,
  symbol,
  name,
  last_price,
  expiry,
  strike,
  tick_size,
  lot_size,
  instrument_type,
  segment,
  exchange,
  fetched_at,
  symbol_full_name
)
SELECT
  instrument_token::int4,
  exchange_token::int4,
  tradingsymbol,
  symbol,
  name,
  last_price::float8,
  expiry::date,
  strike::int8,
  tick_size::float8,
  lot_size::int4,
  instrument_type,
  segment,
  exchange,
  NOW(),
  symbol_full_name
FROM tmp_instruments
"#,
                &[],
            )
            .await?;

        info!(inserted_rows = inserted, elapsed_ms = started.elapsed().as_millis() as u64, "instrument bulk insert done");
        Ok(inserted)
    }
    .await;

    match r {
        Ok(n) => {
            client.batch_execute("COMMIT").await?;
            info!(inserted_rows = n, total_elapsed_ms = started.elapsed().as_millis() as u64, "instrument copy commit");
            Ok(n)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            info!(error = %e, total_elapsed_ms = started.elapsed().as_millis() as u64, "instrument copy rollback");
            Err(e)
        }
    }
}
