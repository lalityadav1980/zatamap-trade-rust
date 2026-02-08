use crate::{core::AppError, db::Db};
use bytes::Bytes;
use futures_util::SinkExt;

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
    println!("InstrumentDAO: BEGIN replace_all_instruments rows={}", instruments.len());
    client.batch_execute("BEGIN").await?;

    let r: Result<u64, AppError> = async {
        let deleted = client.execute("DELETE FROM trade.instrument", &[]).await?;
        println!(
            "InstrumentDAO: deleted_rows={} elapsed_ms={}",
            deleted,
            started.elapsed().as_millis()
        );

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
                println!(
                    "InstrumentDAO: upsert_progress rows={} elapsed_ms={}",
                    n,
                    started.elapsed().as_millis()
                );
            }
        }

        Ok(n)
    }
    .await;

    match r {
        Ok(n) => {
            client.batch_execute("COMMIT").await?;
            println!(
                "InstrumentDAO: COMMIT upserted_rows={} total_elapsed_ms={}",
                n,
                started.elapsed().as_millis()
            );
            Ok(n)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            println!(
                "InstrumentDAO: ROLLBACK error='{}' total_elapsed_ms={}",
                e,
                started.elapsed().as_millis()
            );
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
    println!(
        "InstrumentDAO: BEGIN replace_instruments_by_tokens rows={}",
        instruments.len()
    );
    client.batch_execute("BEGIN").await?;

    let r: Result<u64, AppError> = async {
        let tokens: Vec<i32> = instruments.iter().map(|i| i.instrument_token).collect();
        let deleted = client
            .execute(
                "DELETE FROM trade.instrument WHERE instrument_token = ANY($1)",
                &[&tokens],
            )
            .await?;
        println!(
            "InstrumentDAO: deleted_rows={} elapsed_ms={}",
            deleted,
            started.elapsed().as_millis()
        );

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
                println!(
                    "InstrumentDAO: upsert_progress rows={} elapsed_ms={}",
                    n,
                    started.elapsed().as_millis()
                );
            }
        }
        Ok(n)
    }
    .await;

    match r {
        Ok(n) => {
            client.batch_execute("COMMIT").await?;
            println!(
                "InstrumentDAO: COMMIT upserted_rows={} total_elapsed_ms={}",
                n,
                started.elapsed().as_millis()
            );
            Ok(n)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            println!(
                "InstrumentDAO: ROLLBACK error='{}' total_elapsed_ms={}",
                e,
                started.elapsed().as_millis()
            );
            Err(e)
        }
    }
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
    println!(
        "InstrumentDAO: BEGIN replace_instruments_copy rows={} delete_all={}",
        instruments.len(),
        delete_all
    );
    client.batch_execute("BEGIN").await?;

    let r: Result<u64, AppError> = async {
        if delete_all {
            let deleted = client.execute("DELETE FROM trade.instrument", &[]).await?;
            println!(
                "InstrumentDAO: deleted_rows={} elapsed_ms={}",
                deleted,
                started.elapsed().as_millis()
            );
        } else {
            let tokens: Vec<i32> = instruments.iter().map(|i| i.instrument_token).collect();
            let deleted = client
                .execute(
                    "DELETE FROM trade.instrument WHERE instrument_token = ANY($1)",
                    &[&tokens],
                )
                .await?;
            println!(
                "InstrumentDAO: deleted_rows={} elapsed_ms={}",
                deleted,
                started.elapsed().as_millis()
            );
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
                println!(
                    "InstrumentDAO: copy_progress rows={} elapsed_ms={}",
                    sent_rows,
                    started.elapsed().as_millis()
                );
            }
        }

        if !buf.is_empty() {
            sink.as_mut().send(Bytes::from(buf)).await?;
        }

        let copied_rows = sink.as_mut().finish().await?;
        println!(
            "InstrumentDAO: copy_done rows={} elapsed_ms={}",
            copied_rows,
            started.elapsed().as_millis()
        );

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

        println!(
            "InstrumentDAO: bulk_inserted_rows={} elapsed_ms={}",
            inserted,
            started.elapsed().as_millis()
        );
        Ok(inserted)
    }
    .await;

    match r {
        Ok(n) => {
            client.batch_execute("COMMIT").await?;
            println!(
                "InstrumentDAO: COMMIT inserted_rows={} total_elapsed_ms={}",
                n,
                started.elapsed().as_millis()
            );
            Ok(n)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            println!(
                "InstrumentDAO: ROLLBACK error='{}' total_elapsed_ms={}",
                e,
                started.elapsed().as_millis()
            );
            Err(e)
        }
    }
}
