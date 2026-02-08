use crate::core::AppError;
use std::sync::Arc;
use tokio_postgres::{Client, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;
use rustls::client::{ServerCertVerifier, ServerCertVerified};
use tracing::warn;

pub struct Db {
    client: Arc<Client>,
}

impl Db {
    pub async fn connect(database_url: &str) -> Result<Self, AppError> {
        let (client, connection): (
            Client,
            std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), tokio_postgres::Error>> + Send>>,
        ) = if requires_tls(database_url) {
            let tls = make_rustls_connector(database_url);
            let (client, connection) = tokio_postgres::connect(database_url, tls).await?;
            (client, Box::pin(async move { connection.await }))
        } else {
            let (client, connection) = tokio_postgres::connect(database_url, NoTls).await?;
            (client, Box::pin(async move { connection.await }))
        };

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

fn requires_tls(database_url: &str) -> bool {
    // tokio-postgres accepts keyword/value connection strings.
    // We only enable TLS when explicitly required.
    let lower = database_url.to_ascii_lowercase();
    lower.contains("sslmode=require")
        || lower.contains("sslmode=verify-full")
        || lower.contains("sslmode=verify-ca")
}

fn sslmode(database_url: &str) -> Option<String> {
    // connection string is key=value tokens separated by spaces.
    // Example: "host=... dbname=... sslmode=require"
    for part in database_url.split_whitespace() {
        let mut it = part.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if k.eq_ignore_ascii_case("sslmode") {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_ascii_lowercase());
            }
        }
    }
    None
}

fn parse_bool_env(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"),
        Err(_) => false,
    }
}

fn make_rustls_connector(database_url: &str) -> MakeRustlsConnect {
    let mut roots = rustls::RootCertStore::empty();

    // Prefer native cert store (important for managed DB providers whose CAs are
    // trusted by the OS but may not be in the webpki-roots bundle).
    if let Ok(native) = rustls_native_certs::load_native_certs() {
        for cert in native {
            let _ = roots.add(&rustls::Certificate(cert.0));
        }
    }

    roots.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));
    let mut config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();

    // Some Postgres deployments use a custom/self-signed CA.
    // Libpq's sslmode=require often only enforces encryption, not CA validation.
    // To keep a secure default, we only disable cert verification when the user
    // explicitly opts in.
    if sslmode(database_url).as_deref() == Some("require") && parse_bool_env("PGTLS_SKIP_VERIFY") {
        warn!("DB: WARNING PGTLS_SKIP_VERIFY=1 (TLS cert verification disabled)");
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoCertificateVerification));
    }
    MakeRustlsConnect::new(config)
}

struct NoCertificateVerification;

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
}
