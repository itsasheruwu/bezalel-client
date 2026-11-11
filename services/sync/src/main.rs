mod auth;
mod packs;
mod releases;

use std::{
    env,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bezalel_pack_sync::{Account, MAX_ARTIFACT_BYTES, MAX_SOURCE_BYTES, sha256_hex};
use object_store::{ObjectStore, aws::AmazonS3Builder};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    database: PgPool,
    objects: Arc<dyn ObjectStore>,
    signing_key: Arc<Ed25519KeyPair>,
    admin_digest: Option<Arc<str>>,
    microsoft: Option<Arc<auth::Microsoft>>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "This action requires a different pack role",
        )
    }

    fn missing(message: &'static str) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn conflict(message: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The sync service could not complete the request",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "message": self.message })),
        )
            .into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(code) = error.as_database_error().and_then(|error| error.code()) {
            eprintln!("Sync database request failed (SQLSTATE {code})");
        }
        Self::internal()
    }
}

impl From<object_store::Error> for ApiError {
    fn from(_: object_store::Error) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "Private artifact storage is unavailable",
        )
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn secret() -> ApiResult<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| ApiError::internal())?;
    Ok(hex::encode(bytes))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty() && value.len() <= 256)
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> ApiResult<Account> {
    let unauthorized = || ApiError::new(StatusCode::UNAUTHORIZED, "Connect your Bezalel account");
    let token = bearer(headers).ok_or_else(unauthorized)?;
    let row = sqlx::query(
        "SELECT a.id, a.name FROM sync_sessions s JOIN sync_accounts a ON a.id = s.account_id
         WHERE s.token_sha256 = $1 AND s.expires_at > $2
         UNION ALL
         SELECT a.id, a.name FROM sync_accounts a WHERE $3 AND a.token_sha256 = $1
         LIMIT 1",
    )
    .bind(sha256_hex(token.as_bytes()))
    .bind(now())
    .bind(state.admin_digest.is_some())
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(unauthorized)?;
    Ok(Account {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
    })
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Account>> {
    Ok(Json(authenticate(&state, &headers).await?))
}

#[derive(Deserialize)]
struct NewAccount {
    name: String,
}

#[derive(Serialize)]
struct IssuedAccount {
    account: Account,
    token: String,
}

async fn issue_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<NewAccount>,
) -> ApiResult<Json<IssuedAccount>> {
    let admin_digest = state
        .admin_digest
        .as_ref()
        .ok_or(ApiError::missing("Operator account issuance is disabled"))?;
    let digest = bearer(&headers).map(|token| sha256_hex(token.as_bytes()));
    if !digest
        .as_deref()
        .is_some_and(|digest| digest.as_bytes().ct_eq(admin_digest.as_bytes()).into())
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Operator authorization required",
        ));
    }
    let name = clean_name(&input.name, 48)?;
    let account = Account {
        id: Uuid::new_v4(),
        name,
    };
    let token = secret()?;
    sqlx::query("INSERT INTO sync_accounts (id, name, token_sha256) VALUES ($1, $2, $3)")
        .bind(account.id)
        .bind(&account.name)
        .bind(sha256_hex(token.as_bytes()))
        .execute(&state.database)
        .await?;
    Ok(Json(IssuedAccount { account, token }))
}

fn clean_name(name: &str, limit: usize) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > limit || name.chars().any(char::is_control) {
        return Err(ApiError::bad("Enter a short, readable name"));
    }
    Ok(name.into())
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn router(state: AppState) -> Router {
    use packs::*;
    use releases::*;
    Router::new()
        .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .route("/v1/admin/accounts", post(issue_account))
        .route("/v1/me", get(me))
        .merge(auth::routes())
        .route("/v1/packs", get(list_packs).post(create_pack))
        .route("/v1/invites/preview", post(preview_invite))
        .route("/v1/invites/join", post(join_pack))
        .route(
            "/v1/packs/{pack}/invites",
            get(list_invites).post(create_invite),
        )
        .route("/v1/packs/{pack}/invites/{invite}", delete(revoke_invite))
        .route("/v1/packs/{pack}/members", get(list_members))
        .route("/v1/packs/{pack}/members/{member}", delete(remove_member))
        .route(
            "/v1/packs/{pack}/submissions",
            get(list_submissions)
                .post(submit_mod)
                .layer(DefaultBodyLimit::max(
                    MAX_ARTIFACT_BYTES * 4 / 3 + MAX_SOURCE_BYTES + 128 * 1024,
                )),
        )
        .route(
            "/v1/packs/{pack}/candidates",
            get(list_candidates).post(create_candidate),
        )
        .route("/v1/packs/{pack}/publish", post(publish))
        .route("/v1/packs/{pack}/config", post(submit_config))
        .route("/v1/packs/{pack}/releases", get(history))
        .route("/v1/packs/{pack}/active", get(active))
        .route("/v1/packs/{pack}/artifacts/{sha256}", get(artifact))
        .layer(middleware::from_fn(no_store))
        .with_state(state)
}

fn signing_key() -> Result<Ed25519KeyPair, String> {
    let input = env::var("BEZALEL_SYNC_SIGNING_KEY").map_err(|_| "Set BEZALEL_SYNC_SIGNING_KEY")?;
    let seed: [u8; 32] = hex::decode(input.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("BEZALEL_SYNC_SIGNING_KEY must be a 32-byte hexadecimal seed")?;
    Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| "Invalid signing key".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match env::args().nth(1).as_deref() {
        Some("--generate-key") => {
            let mut bytes = [0; 32];
            getrandom::fill(&mut bytes).map_err(|_| "Could not generate a random key")?;
            let key = Ed25519KeyPair::from_seed_unchecked(&bytes)
                .map_err(|_| "Could not load signing key")?;
            println!("private seed: {}", hex::encode(bytes));
            println!("public key:   {}", hex::encode(key.public_key().as_ref()));
            return Ok(());
        }
        Some("--public-key") => {
            println!("{}", hex::encode(signing_key()?.public_key().as_ref()));
            return Ok(());
        }
        Some(_) => return Err("Unknown option".into()),
        None => {}
    }
    let key = signing_key()?;
    let microsoft = auth::Microsoft::from_env()?.map(Arc::new);
    let address = match (env::var("BEZALEL_SYNC_LISTEN"), env::var("PORT")) {
        (Ok(address), _) => address,
        (Err(_), Ok(port)) => format!("0.0.0.0:{}", port.parse::<u16>()?),
        _ => "127.0.0.1:3748".into(),
    };
    let database = PgPoolOptions::new()
        .max_connections(8)
        .connect(&env::var("DATABASE_URL").map_err(|_| "Set DATABASE_URL")?)
        .await
        .map_err(|_| "Could not connect to PostgreSQL")?;
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(&database)
        .await?;
    match env::var("BEZALEL_OWNER_ACCESS_CODE_SHA256") {
        Ok(digest) => auth::bootstrap_owner(&database, &digest).await?,
        Err(env::VarError::NotPresent) => {}
        Err(_) => {
            return Err(
                "BEZALEL_OWNER_ACCESS_CODE_SHA256 must be a hexadecimal SHA-256 digest".into(),
            );
        }
    }
    let bucket = env::var("BEZALEL_S3_BUCKET").map_err(|_| "Set BEZALEL_S3_BUCKET")?;
    let objects = AmazonS3Builder::from_env()
        .with_bucket_name(bucket)
        .build()
        .map_err(|_| "Could not configure S3-compatible artifact storage")?;
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let operator_token = env::var("BEZALEL_ADMIN_TOKEN").ok();
    let operator_enabled = match env::var("BEZALEL_ENABLE_OPERATOR_TOKENS").as_deref() {
        Ok("true") => true,
        Ok("false") => false,
        Err(_) => listener.local_addr()?.ip().is_loopback() && operator_token.is_some(),
        Ok(_) => return Err("BEZALEL_ENABLE_OPERATOR_TOKENS must be true or false".into()),
    };
    let admin_digest = if operator_enabled {
        let token = operator_token.ok_or("Set BEZALEL_ADMIN_TOKEN to enable operator accounts")?;
        if !(32..=256).contains(&token.len()) || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err("Use a random 32–256 character BEZALEL_ADMIN_TOKEN".into());
        }
        Some(Arc::from(sha256_hex(token.as_bytes())))
    } else {
        None
    };
    eprintln!("Bezalel sync listening on {}", listener.local_addr()?);
    let state = AppState {
        database,
        objects: Arc::new(objects),
        signing_key: Arc::new(key),
        admin_digest,
        microsoft,
    };
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
