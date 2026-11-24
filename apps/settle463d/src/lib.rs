//! Loopback HTTP coordinator for the simulator deployment.
mod actions;
mod db;
mod http;
#[cfg(test)]
mod tests;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json, Router,
};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use settle463_core::{
    canonical,
    ident::{environment, hex, hex_string},
    message_id,
    settlement::FundingError,
};
use sqlx::{
    migrate::{Migration, MigrationType, Migrator},
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::{path::PathBuf, sync::Arc};
use tokio::net::TcpListener;

type BoxError = Box<dyn std::error::Error>;
type Result<T> = std::result::Result<T, ApiError>;
type Conn = sqlx::SqliteConnection;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Simulator genesis and local storage. Existing databases pin their genesis and receipt issuer.
pub struct Config {
    pub database: PathBuf,
    pub artifact_root: PathBuf,
    pub environment: String,
    pub clock: i64,
    pub buyer: String,
    pub opening_balance: u64,
    pub issuer_seed_file: PathBuf,
    pub admin_token: String,
}
struct State {
    pool: SqlitePool,
    config: Config,
    key: SigningKey,
}
type Shared = Arc<State>;

#[derive(Debug, Clone)]
struct ApiError(StatusCode, &'static str);
impl ApiError {
    fn bad(code: &'static str) -> Self {
        Self(StatusCode::BAD_REQUEST, code)
    }
    fn conflict(code: &'static str) -> Self {
        Self(StatusCode::CONFLICT, code)
    }
    fn link() -> Self {
        Self(StatusCode::UNPROCESSABLE_ENTITY, "LINKAGE_FAILED")
    }
    fn denied(code: &'static str) -> Self {
        Self(StatusCode::FORBIDDEN, code)
    }
    fn auth() -> Self {
        Self(StatusCode::UNAUTHORIZED, "BAD_CHALLENGE")
    }
    fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "NOT_FOUND")
    }
    fn store() -> Self {
        Self(StatusCode::SERVICE_UNAVAILABLE, "STORE_UNAVAILABLE")
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.1)
    }
}
impl std::error::Error for ApiError {}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":{"code":self.1,"message":self.1}})),
        )
            .into_response()
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::missing(),
            _ => {
                eprintln!("coordinator database: {error}");
                Self::store()
            }
        }
    }
}
impl From<serde_json::Error> for ApiError {
    fn from(_: serde_json::Error) -> Self {
        Self::bad("MALFORMED_JSON")
    }
}
impl From<settle463_core::Error> for ApiError {
    fn from(e: settle463_core::Error) -> Self {
        use settle463_core::Error;
        match e {
            Error::Signature => Self(StatusCode::UNAUTHORIZED, "BAD_SIGNATURE"),
            Error::Invalid("pinned environment") => {
                Self(StatusCode::UNPROCESSABLE_ENTITY, "ENVIRONMENT_MISMATCH")
            }
            Error::Invalid("protocol or version") => Self::bad("UNSUPPORTED_VERSION"),
            Error::Invalid(_) => Self::bad("BAD_ENCODING"),
            Error::Json(j) => {
                let m = j.to_string();
                Self::bad(if m.contains("duplicate") {
                    "DUPLICATE_FIELD"
                } else if m.contains("unknown field") {
                    "UNKNOWN_FIELD"
                } else if m.contains("unknown variant") {
                    "UNSUPPORTED_KIND"
                } else {
                    "MALFORMED_JSON"
                })
            }
        }
    }
}
impl From<settle463_core::receipt::ReceiptError> for ApiError {
    fn from(_: settle463_core::receipt::ReceiptError) -> Self {
        Self::link()
    }
}
impl From<FundingError> for ApiError {
    fn from(e: FundingError) -> Self {
        match e {
            FundingError::Wire(w) => w.into(),
            FundingError::AlreadyFunded => Self::conflict("ALREADY_COMPLETE"),
            FundingError::Linkage => Self::link(),
            FundingError::Environment => {
                Self(StatusCode::UNPROCESSABLE_ENTITY, "ENVIRONMENT_MISMATCH")
            }
            FundingError::Unavailable => {
                Self(StatusCode::SERVICE_UNAVAILABLE, "LEDGER_UNAVAILABLE")
            }
            _ => Self::conflict("STATE_CONFLICT"),
        }
    }
}

fn digest(data: &[u8]) -> String {
    hex_string(&message_id(data))
}
fn random() -> Result<String> {
    let mut b = [0; 32];
    getrandom::getrandom(&mut b).map_err(|_| ApiError::store())?;
    Ok(hex_string(&b))
}
fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or(ApiError::bad("BAD_ENCODING"))
}
fn preimage(domain: &[u8], v: &Value) -> Result<Vec<u8>> {
    let mut b = domain.to_vec();
    b.push(0);
    b.extend(canonical::canonical(v)?);
    Ok(b)
}
fn json_bytes(v: Value) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&v)?)
}

async fn open(c: Config) -> std::result::Result<Shared, BoxError> {
    environment(&c.environment)?;
    if !c.environment.starts_with("sim:") || c.clock < 0 {
        return Err("invalid simulator environment or clock".into());
    }
    hex::<32>(&c.buyer)?;
    hex::<32>(&c.admin_token)?;
    tokio::fs::create_dir_all(&c.artifact_root).await?;
    let key = SigningKey::from_bytes(&hex(tokio::fs::read_to_string(&c.issuer_seed_file)
        .await?
        .trim())?);
    let options = SqliteConnectOptions::new()
        .filename(&c.database)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let migrations = vec![Migration::new(
        1,
        "coordinator".into(),
        MigrationType::Simple,
        include_str!("../migrations/0001_coordinator.sql").into(),
        false,
    )];
    Migrator {
        migrations: migrations.into(),
        ..Migrator::DEFAULT
    }
    .run(&pool)
    .await?;
    let issuer = hex_string(key.verifying_key().as_bytes());
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("INSERT OR IGNORE INTO deployment VALUES(1,?,?,?,?,?,?)")
        .bind(&c.environment)
        .bind(c.clock.to_string())
        .bind(c.clock.to_string())
        .bind(&c.buyer)
        .bind(c.opening_balance.to_string())
        .bind(&issuer)
        .execute(&mut *tx)
        .await?;
    let matches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM deployment WHERE environment=? AND genesis_clock=? AND buyer=?
         AND opening_balance=? AND receipt_issuer=?",
    )
    .bind(&c.environment)
    .bind(c.clock.to_string())
    .bind(&c.buyer)
    .bind(c.opening_balance.to_string())
    .bind(issuer)
    .fetch_one(&mut *tx)
    .await?;
    if matches != 1 {
        return Err("existing database belongs to a different genesis or receipt issuer".into());
    }
    tx.commit().await?;
    let state = Arc::new(State {
        pool,
        config: c,
        key,
    });
    actions::recover(&state).await?;
    Ok(state)
}

/// Recovers committed intents before announcing the bound loopback address.
pub async fn serve(config: Config, listener: TcpListener) -> std::result::Result<(), BoxError> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err("simulator listener requires loopback".into());
    }
    let state = open(config).await?;
    let app = Router::new()
        .fallback(http::dispatch)
        .with_state(state.clone());
    let worker = tokio::spawn(async move {
        // The crash test must terminate at the staging boundary before any worker can broadcast.
        #[cfg(debug_assertions)]
        if std::env::var("SETTLE463_TEST_STOP").as_deref() == Ok("before_broadcast") {
            return;
        }
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if let Err(error) = actions::recover(&state).await {
                eprintln!("outbox recovery: {error}");
            }
        }
    });
    println!("LISTEN={}", listener.local_addr()?);
    let result = axum::serve(listener, app).await;
    worker.abort();
    Ok(result?)
}
/// Runs the standalone daemon with `--config FILE` and optional `--listen LOOPBACK:PORT`.
pub fn run_cli(args: &[String]) -> std::result::Result<(), BoxError> {
    let mut config = None;
    let mut address = "127.0.0.1:0".to_owned();
    for pair in args.chunks(2) {
        let [flag, value] = pair else {
            return Err("usage: settle463d --config FILE [--listen LOOPBACK:PORT]".into());
        };
        match flag.as_str() {
            "--config" => config = Some(value),
            "--listen" => address.clone_from(value),
            _ => return Err("unknown coordinator option".into()),
        }
    }
    let cfg: Config = serde_json::from_slice(&std::fs::read(config.ok_or("missing --config")?)?)?;
    let addr: std::net::SocketAddr = address.parse()?;
    if !addr.ip().is_loopback() {
        return Err("simulator listener requires loopback".into());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async { serve(cfg, TcpListener::bind(addr).await?).await })
}
