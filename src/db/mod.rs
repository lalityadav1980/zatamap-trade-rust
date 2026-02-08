use crate::core::AppError;
use std::sync::Arc;
use tokio_postgres::{Client, NoTls};

pub struct Db {
    client: Arc<Client>,
}

impl Db {
    pub async fn connect(database_url: &str) -> Result<Self, AppError> {
        let (client, connection) = tokio_postgres::connect(database_url, NoTls).await?;
        tokio::spawn(async move {
            let _ = connection.await;
        });

        client
            .batch_execute(
                "\
                SET search_path TO trade,public;
                SET TIME ZONE 'UTC';
                SET client_encoding = 'UTF8';
                ",
            )
            .await?;

        Ok(Self {
            client: Arc::new(client),
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub async fn health(&self) -> Result<bool, AppError> {
        let row = self.client.query_one("SELECT 1", &[]).await?;
        let v: i32 = row.get(0);
        Ok(v == 1)
    }
}
