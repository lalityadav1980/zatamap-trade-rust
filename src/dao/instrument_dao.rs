use crate::{core::AppError, db::Db};

#[derive(Debug, Clone)]
pub struct InstrumentUpsert {
    pub instrument_token: i32,
    pub exchange_token: Option<i32>,
    pub tradingsymbol: Option<String>,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub last_price: Option<String>,
    pub expiry: Option<String>,
    pub strike: Option<i64>,
    pub tick_size: Option<String>,
    pub lot_size: Option<i32>,
    pub instrument_type: Option<String>,
    pub segment: Option<String>,
    pub exchange: Option<String>,
    pub symbol_full_name: Option<String>,
}

pub async fn replace_all_instruments(db: &Db, instruments: &[InstrumentUpsert]) -> Result<u64, AppError> {
    let client = db.client();
    client.batch_execute("BEGIN").await?;

    let r: Result<u64, AppError> = async {
        client.execute("DELETE FROM trade.instrument", &[]).await?;

        let stmt = client
            .prepare(
            "INSERT INTO trade.instrument (\
                instrument_token, exchange_token, tradingsymbol, symbol, name, last_price,\
                expiry, strike, tick_size, lot_size, instrument_type, segment, exchange, fetched_at, symbol_full_name\
            ) VALUES (\
                $1, $2, $3, $4, $5, $6::numeric,\
                $7::date, $8::numeric, $9::numeric, $10, $11, $12, $13, NOW(), $14\
            )\
            ON CONFLICT (instrument_token) DO UPDATE SET\
                exchange_token   = EXCLUDED.exchange_token,\
                tradingsymbol    = EXCLUDED.tradingsymbol,\
                symbol           = EXCLUDED.symbol,\
                name             = EXCLUDED.name,\
                last_price       = EXCLUDED.last_price,\
                expiry           = EXCLUDED.expiry,\
                strike           = EXCLUDED.strike,\
                tick_size        = EXCLUDED.tick_size,\
                lot_size         = EXCLUDED.lot_size,\
                instrument_type  = EXCLUDED.instrument_type,\
                segment          = EXCLUDED.segment,\
                exchange         = EXCLUDED.exchange,\
                fetched_at       = NOW(),\
                symbol_full_name = EXCLUDED.symbol_full_name",
            )
            .await?;

        let mut n: u64 = 0;
        for i in instruments {
            let last_price = i.last_price.as_deref();
            let expiry = i.expiry.as_deref();
            let tick_size = i.tick_size.as_deref();
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
                        &last_price,
                        &expiry,
                        &i.strike,
                        &tick_size,
                        &i.lot_size,
                        &i.instrument_type,
                        &i.segment,
                        &i.exchange,
                        &symbol_full_name,
                    ],
                )
                .await?;
        }

        Ok(n)
    }
    .await;

    match r {
        Ok(n) => {
            client.batch_execute("COMMIT").await?;
            Ok(n)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            Err(e)
        }
    }
}
