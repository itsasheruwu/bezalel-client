mod access_code;

use std::{env, time::Duration};

use axum::{
    Form, Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::Redirect,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bezalel_pack_sync::sha256_hex;
use reqwest::{Client, Url};
use ring::{digest, signature};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{PgConnection, Row, postgres::PgRow};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{ApiError, ApiResult, AppState, authenticate, bearer, clean_name, now, secret};
pub(super) use access_code::bootstrap_owner;

const AUTHORIZE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const JWKS: &str = "https://login.microsoftonline.com/consumers/discovery/v2.0/keys";
const CONSUMER_ISSUER: &str =
    "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0";
const TENANT_KEY_ISSUER: &str = "https://login.microsoftonline.com/{tenantid}/v2.0";
const CALLBACK_PATH: &str = "/v1/auth/microsoft/callback";
const DESKTOP_PATH: &str = "/bezalel/signin";
const FLOW_TTL: i64 = 10 * 60;
const HANDOFF_TTL: i64 = 60;
const SESSION_TTL: i64 = 30 * 24 * 60 * 60;
const MAX_FLOWS: i64 = 1024;
const MAX_PROVIDER_RESPONSE: usize = 128 * 1024;

pub(super) struct Microsoft {
    client: Client,
    client_id: String,
    client_secret: String,
    redirect_url: Url,
}

impl Microsoft {
    pub(super) fn from_env() -> Result<Option<Self>, String> {
        let id = env::var("BEZALEL_MICROSOFT_OIDC_CLIENT_ID").ok();
        let secret = env::var("BEZALEL_MICROSOFT_OIDC_CLIENT_SECRET").ok();
        let redirect = env::var("BEZALEL_MICROSOFT_OIDC_REDIRECT_URL").ok();
        let (client_id, client_secret, redirect_url) = match (id, secret, redirect) {
            (None, None, None) => return Ok(None),
            (Some(id), Some(secret), Some(redirect)) => (id, secret, redirect),
            _ => return Err("Set all three BEZALEL_MICROSOFT_OIDC variables together".into()),
        };
        let client_id = Uuid::parse_str(&client_id)
            .map_err(|_| "Microsoft OIDC client ID must be an application UUID")?
            .to_string();
        if client_secret.is_empty()
            || client_secret.len() > 4096
            || client_secret.chars().any(char::is_control)
        {
            return Err("Invalid Microsoft OIDC client secret".into());
        }
        let redirect_url =
            Url::parse(&redirect_url).map_err(|_| "Invalid Microsoft OIDC callback URL")?;
        if redirect_url.scheme() != "https"
            || redirect_url.host_str().is_none()
            || !redirect_url.username().is_empty()
            || redirect_url.password().is_some()
            || redirect_url.path() != CALLBACK_PATH
            || redirect_url.query().is_some()
            || redirect_url.fragment().is_some()
        {
            return Err(format!(
                "Microsoft OIDC callback must be an HTTPS URL ending in {CALLBACK_PATH}"
            ));
        }
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "Could not initialize Microsoft sign-in")?;
        Ok(Some(Self {
            client,
            client_id,
            client_secret,
            redirect_url,
        }))
    }

    async fn identity(&self, code: &str, flow: &Flow) -> ApiResult<Claims> {
        let token: TokenResponse = provider_json(
            self.client
                .post(TOKEN)
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", &self.client_id),
                    ("client_secret", &self.client_secret),
                    ("code", code),
                    ("redirect_uri", self.redirect_url.as_str()),
                    ("code_verifier", &flow.microsoft_verifier),
                ])
                .send()
                .await,
        )
        .await?;
        let keys: KeySet = provider_json(self.client.get(JWKS).send().await).await?;
        verified_claims(
            &token.id_token,
            &keys.keys,
            &self.client_id,
            &flow.microsoft_nonce,
            now(),
        )
        .ok_or_else(provider_error)
    }
}

fn provider_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "Microsoft sign-in could not be verified",
    )
}

async fn provider_json<T: DeserializeOwned>(
    response: reqwest::Result<reqwest::Response>,
) -> ApiResult<T> {
    let mut response = response.map_err(|_| provider_error())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MAX_PROVIDER_RESPONSE as u64)
    {
        return Err(provider_error());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| provider_error())? {
        if body.len() + chunk.len() > MAX_PROVIDER_RESPONSE {
            return Err(provider_error());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| provider_error())
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Key>,
}

#[derive(Deserialize)]
struct Key {
    kid: String,
    kty: String,
    #[serde(rename = "use")]
    usage: Option<String>,
    issuer: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: String,
    sub: String,
    nonce: String,
    exp: i64,
    nbf: Option<i64>,
}

fn verified_claims(
    id_token: &str,
    keys: &[Key],
    client_id: &str,
    nonce: &str,
    time: i64,
) -> Option<Claims> {
    if id_token.len() > 24 * 1024 {
        return None;
    }
    let (input, signature) = id_token.rsplit_once('.')?;
    let (header, claims) = input.split_once('.')?;
    let header: JwtHeader = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    if header.alg != "RS256" || header.kid.is_empty() || header.kid.len() > 160 {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    let signed = keys.iter().any(|key| {
        if key.kid != header.kid
            || key.kty != "RSA"
            || key.usage.as_deref().is_some_and(|usage| usage != "sig")
            || key
                .issuer
                .as_deref()
                .is_some_and(|issuer| !matches!(issuer, CONSUMER_ISSUER | TENANT_KEY_ISSUER))
        {
            return false;
        }
        let (Some(n), Some(e)) = (&key.n, &key.e) else {
            return false;
        };
        let (Ok(n), Ok(e)) = (URL_SAFE_NO_PAD.decode(n), URL_SAFE_NO_PAD.decode(e)) else {
            return false;
        };
        signature::RsaPublicKeyComponents { n: &n, e: &e }
            .verify(
                &signature::RSA_PKCS1_2048_8192_SHA256,
                input.as_bytes(),
                &signature,
            )
            .is_ok()
    });
    if !signed {
        return None;
    }
    let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).ok()?).ok()?;
    claims_valid(&claims, client_id, nonce, time).then_some(claims)
}

fn claims_valid(claims: &Claims, client_id: &str, nonce: &str, time: i64) -> bool {
    claims.iss == CONSUMER_ISSUER
        && claims.aud == client_id
        && bool::from(claims.nonce.as_bytes().ct_eq(nonce.as_bytes()))
        && claims.exp > time
        && claims.nbf.is_none_or(|nbf| nbf <= time + 60)
        && (1..=255).contains(&claims.sub.len())
        && claims.sub.bytes().all(|byte| byte.is_ascii_graphic())
}

fn desktop_callback(value: &str) -> Option<Url> {
    if value.len() > 96 {
        return None;
    }
    let url = Url::parse(value).ok()?;
    let port = url.port().filter(|port| *port >= 1024)?;
    if value == format!("http://127.0.0.1:{port}{DESKTOP_PATH}") {
        Some(url)
    } else {
        None
    }
}

fn urlsafe(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_challenge(value: &str) -> bool {
    urlsafe(value, 43, 43)
        && URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|bytes| bytes.len() == 32)
}

fn valid_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'~'))
}

fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()))
}

fn is_secret(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest {
    callback_url: String,
    challenge: String,
    state: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartResponse {
    authorize_url: String,
}

async fn start(
    State(state): State<AppState>,
    Json(input): Json<StartRequest>,
) -> ApiResult<Json<StartResponse>> {
    let microsoft = configured(&state)?;
    let callback = desktop_callback(&input.callback_url).ok_or_else(invalid_login)?;
    if !valid_challenge(&input.challenge) || !urlsafe(&input.state, 22, 128) {
        return Err(invalid_login());
    }
    let provider_state = secret()?;
    let provider_verifier = secret()?;
    let nonce = secret()?;
    let time = now();
    let mut transaction = state.database.begin().await?;
    // Serialize pruning and admission so concurrent starts cannot exhaust storage.
    sqlx::query("SELECT pg_advisory_xact_lock(1113953728)")
        .execute(&mut *transaction)
        .await?;
    for query in [
        "DELETE FROM sync_login_flows WHERE expires_at <= $1",
        "DELETE FROM sync_login_handoffs WHERE expires_at <= $1",
        "DELETE FROM sync_sessions WHERE expires_at <= $1",
    ] {
        sqlx::query(query)
            .bind(time)
            .execute(&mut *transaction)
            .await?;
    }
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_login_flows")
        .fetch_one(&mut *transaction)
        .await?;
    if active >= MAX_FLOWS {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many sign-ins are waiting. Try again shortly.",
        ));
    }
    sqlx::query(
        "INSERT INTO sync_login_flows
         (state_sha256, callback_url, desktop_state, desktop_challenge, microsoft_verifier, microsoft_nonce, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(sha256_hex(provider_state.as_bytes()))
    .bind(callback.as_str())
    .bind(input.state)
    .bind(input.challenge)
    .bind(&provider_verifier)
    .bind(&nonce)
    .bind(time + FLOW_TTL)
    .execute(&mut *transaction).await?;
    transaction.commit().await?;
    let mut authorize = Url::parse(AUTHORIZE).map_err(|_| ApiError::internal())?;
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", microsoft.client_id.as_str()),
        ("redirect_uri", microsoft.redirect_url.as_str()),
        ("response_type", "code"),
        ("response_mode", "form_post"),
        ("scope", "openid"),
        ("state", provider_state.as_str()),
        ("nonce", nonce.as_str()),
        ("code_challenge_method", "S256"),
        ("code_challenge", &s256(&provider_verifier)),
        ("prompt", "select_account"),
    ]);
    Ok(Json(StartResponse {
        authorize_url: authorize.into(),
    }))
}

fn invalid_login() -> ApiError {
    ApiError::bad("Invalid or expired sign-in. Start Microsoft sign-in again.")
}

fn configured(state: &AppState) -> ApiResult<&Microsoft> {
    state.microsoft.as_deref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Microsoft sign-in is not configured on this Bezalel service",
        )
    })
}

#[derive(Deserialize)]
struct CallbackQuery {
    state: String,
    code: Option<String>,
    error: Option<String>,
}

struct Flow {
    callback: Url,
    desktop_state: String,
    desktop_challenge: String,
    microsoft_verifier: String,
    microsoft_nonce: String,
}

impl Flow {
    fn from_row(row: PgRow) -> ApiResult<Self> {
        let callback: String = row.try_get("callback_url")?;
        Ok(Self {
            callback: desktop_callback(&callback).ok_or_else(ApiError::internal)?,
            desktop_state: row.try_get("desktop_state")?,
            desktop_challenge: row.try_get("desktop_challenge")?,
            microsoft_verifier: row.try_get("microsoft_verifier")?,
            microsoft_nonce: row.try_get("microsoft_nonce")?,
        })
    }

    fn response(mut self, key: &str, value: &str) -> Redirect {
        self.callback
            .query_pairs_mut()
            .append_pair("state", &self.desktop_state)
            .append_pair(key, value);
        Redirect::to(self.callback.as_str())
    }
}

async fn callback(
    State(state): State<AppState>,
    Form(input): Form<CallbackQuery>,
) -> ApiResult<Redirect> {
    let microsoft = configured(&state)?;
    if !is_secret(&input.state) {
        return Err(invalid_login());
    }
    let row = sqlx::query(
        "DELETE FROM sync_login_flows WHERE state_sha256 = $1 AND expires_at > $2
         RETURNING callback_url, desktop_state, desktop_challenge, microsoft_verifier, microsoft_nonce",
    )
    .bind(sha256_hex(input.state.as_bytes()))
    .bind(now())
    .fetch_optional(&state.database).await?
    .ok_or_else(invalid_login)?;
    let flow = Flow::from_row(row)?;
    if let Some(error) = input.error {
        let reason = if error == "access_denied" {
            "access_denied"
        } else {
            "microsoft_error"
        };
        return Ok(flow.response("error", reason));
    }
    let Some(code) = input.code.filter(|code| {
        (1..=8192).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_graphic())
    }) else {
        return Ok(flow.response("error", "microsoft_error"));
    };
    let handoff = match microsoft.identity(&code, &flow).await {
        Ok(identity) => create_handoff(&state, &flow, &identity).await,
        Err(error) => Err(error),
    };
    match handoff {
        Ok(code) => Ok(flow.response("code", &code)),
        Err(_) => {
            eprintln!("Microsoft sign-in could not complete");
            Ok(flow.response("error", "microsoft_error"))
        }
    }
}

async fn create_handoff(state: &AppState, flow: &Flow, claims: &Claims) -> ApiResult<String> {
    let mut transaction = state.database.begin().await?;
    let account_id: Uuid = sqlx::query_scalar(
        "INSERT INTO sync_accounts (id, name, microsoft_issuer, microsoft_subject, profile_complete)
         VALUES ($1, 'Player', $2, $3, false)
         ON CONFLICT (microsoft_issuer, microsoft_subject) DO UPDATE
         SET microsoft_subject = EXCLUDED.microsoft_subject RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(&claims.iss)
    .bind(&claims.sub)
    .fetch_one(&mut *transaction).await?;
    let code = secret()?;
    sqlx::query(
        "INSERT INTO sync_login_handoffs (code_sha256, account_id, desktop_challenge, expires_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(sha256_hex(code.as_bytes()))
    .bind(account_id)
    .bind(&flow.desktop_challenge)
    .bind(now() + HANDOFF_TTL)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(code)
}

#[derive(Deserialize)]
struct ExchangeRequest {
    code: String,
    verifier: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeResponse {
    token: String,
    expires_at: i64,
    profile: Profile,
}

async fn exchange(
    State(state): State<AppState>,
    Json(input): Json<ExchangeRequest>,
) -> ApiResult<Json<ExchangeResponse>> {
    configured(&state)?;
    if !is_secret(&input.code) || !valid_verifier(&input.verifier) {
        return Err(invalid_login());
    }
    let mut transaction = state.database.begin().await?;
    let time = now();
    let id: Uuid = sqlx::query_scalar(
        "DELETE FROM sync_login_handoffs WHERE code_sha256 = $1
         AND desktop_challenge = $2 AND expires_at > $3 RETURNING account_id",
    )
    .bind(sha256_hex(input.code.as_bytes()))
    .bind(s256(&input.verifier))
    .bind(time)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(invalid_login)?;
    let session = create_session(&mut transaction, id, time).await?;
    transaction.commit().await?;
    Ok(Json(session))
}

async fn create_session(
    database: &mut PgConnection,
    id: Uuid,
    time: i64,
) -> ApiResult<ExchangeResponse> {
    let token = secret()?;
    let expires_at = time + SESSION_TTL;
    sqlx::query("DELETE FROM sync_sessions WHERE expires_at <= $1")
        .bind(time)
        .execute(&mut *database)
        .await?;
    sqlx::query(
        "INSERT INTO sync_sessions (token_sha256, account_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(sha256_hex(token.as_bytes()))
    .bind(id)
    .bind(expires_at)
    .execute(&mut *database)
    .await?;
    let row = sqlx::query(
        "SELECT id, name, avatar, profile_complete, can_invite FROM sync_accounts WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *database)
    .await?;
    let profile = Profile::from_row(row)?;
    Ok(ExchangeResponse {
        token,
        expires_at,
        profile,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    id: Uuid,
    name: String,
    avatar: String,
    setup_complete: bool,
    can_invite: bool,
}

impl Profile {
    fn from_row(row: PgRow) -> ApiResult<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            avatar: row.try_get("avatar")?,
            setup_complete: row.try_get("profile_complete")?,
            can_invite: row.try_get("can_invite")?,
        })
    }
}

async fn profile(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Profile>> {
    let account = authenticate(&state, &headers).await?;
    let row = sqlx::query(
        "SELECT id, name, avatar, profile_complete, can_invite FROM sync_accounts WHERE id = $1",
    )
    .bind(account.id)
    .fetch_one(&state.database)
    .await?;
    Ok(Json(Profile::from_row(row)?))
}

#[derive(Deserialize)]
struct ProfileUpdate {
    name: String,
    avatar: String,
}

async fn update_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ProfileUpdate>,
) -> ApiResult<Json<Profile>> {
    let account = authenticate(&state, &headers).await?;
    let name = clean_name(&input.name, 48)?;
    if !matches!(input.avatar.as_str(), "violet" | "lime" | "ember" | "sky") {
        return Err(ApiError::bad("Choose one of the available profile colors"));
    }
    let row = sqlx::query(
        "UPDATE sync_accounts SET name = $2, avatar = $3, profile_complete = true
         WHERE id = $1 RETURNING id, name, avatar, profile_complete, can_invite",
    )
    .bind(account.id)
    .bind(name)
    .bind(input.avatar)
    .fetch_one(&state.database)
    .await?;
    Ok(Json(Profile::from_row(row)?))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<StatusCode> {
    if let Some(token) = bearer(&headers) {
        sqlx::query("DELETE FROM sync_sessions WHERE token_sha256 = $1")
            .bind(sha256_hex(token.as_bytes()))
            .execute(&state.database)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/microsoft/start", post(start))
        .route(CALLBACK_PATH, post(callback))
        .route("/v1/auth/microsoft/exchange", post(exchange))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/me/profile", get(profile).patch(update_profile))
        .merge(access_code::routes())
        .layer(DefaultBodyLimit::max(8 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_callbacks_only_reach_the_canonical_loopback_path() {
        assert!(desktop_callback("http://127.0.0.1:49327/bezalel/signin").is_some());
        for invalid in [
            "http://localhost:49327/bezalel/signin",
            "http://127.0.0.1:49327@elsewhere/bezalel/signin",
            "http://127.0.0.1:80/bezalel/signin",
            "http://127.0.0.1:49327/other/../bezalel/signin",
            "http://127.0.0.1:49327/bezalel/signin?redirect=https://example.org",
        ] {
            assert!(desktop_callback(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn pkce_matches_the_rfc_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(valid_verifier(verifier));
        assert!(valid_challenge(challenge));
        assert_eq!(s256(verifier), challenge);
        assert!(!valid_verifier("too-short"));
        assert!(!valid_challenge(&format!("{challenge}=")));
    }

    #[test]
    fn claims_require_the_expected_microsoft_identity_and_nonce() {
        let mut claims = Claims {
            iss: CONSUMER_ISSUER.into(),
            aud: "client".into(),
            sub: "subject".into(),
            nonce: "nonce".into(),
            exp: 300,
            nbf: Some(100),
        };
        assert!(claims_valid(&claims, "client", "nonce", 200));
        assert!(!claims_valid(&claims, "other", "nonce", 200));
        assert!(!claims_valid(&claims, "client", "old nonce", 200));
        assert!(!claims_valid(&claims, "client", "nonce", 300));
        claims.iss = "https://example.org".into();
        assert!(!claims_valid(&claims, "client", "nonce", 200));
    }

    #[tokio::test]
    #[ignore = "requires a disposable PostgreSQL database via BEZALEL_AUTH_TEST_DATABASE_URL"]
    async fn hosted_account_rejects_handoff_replay_and_revokes_session() {
        use axum::http::{HeaderValue, header};
        use std::sync::Arc;

        let database = sqlx::PgPool::connect(
            &env::var("BEZALEL_AUTH_TEST_DATABASE_URL").expect("set disposable database URL"),
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE sync_accounts (id uuid PRIMARY KEY, name text NOT NULL, token_sha256 text UNIQUE NOT NULL)",
        )
        .execute(&database).await.unwrap();
        sqlx::query("INSERT INTO sync_accounts VALUES ($1, 'Legacy player', $2)")
            .bind(Uuid::new_v4())
            .bind(sha256_hex(b"legacy-fixture-token"))
            .execute(&database)
            .await
            .unwrap();
        for _ in 0..2 {
            sqlx::raw_sql(include_str!("schema.sql"))
                .execute(&database)
                .await
                .unwrap();
        }
        let state = AppState {
            database,
            objects: Arc::new(object_store::memory::InMemory::new()),
            signing_key: Arc::new(
                signature::Ed25519KeyPair::from_seed_unchecked(&[5; 32]).unwrap(),
            ),
            admin_digest: None,
            microsoft: Some(Arc::new(Microsoft {
                client: Client::new(),
                client_id: Uuid::new_v4().to_string(),
                client_secret: "fixture".into(),
                redirect_url: Url::parse("https://example.invalid/v1/auth/microsoft/callback")
                    .unwrap(),
            })),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer legacy-fixture-token"),
        );
        assert!(authenticate(&state, &headers).await.is_err());

        let verifier = secret().unwrap();
        let flow = Flow {
            callback: desktop_callback("http://127.0.0.1:43928/bezalel/signin").unwrap(),
            desktop_state: secret().unwrap(),
            desktop_challenge: s256(&verifier),
            microsoft_verifier: secret().unwrap(),
            microsoft_nonce: secret().unwrap(),
        };
        let Json(started) = start(
            State(state.clone()),
            Json(StartRequest {
                callback_url: flow.callback.to_string(),
                challenge: flow.desktop_challenge.clone(),
                state: flow.desktop_state.clone(),
            }),
        )
        .await
        .unwrap();
        let authorize = Url::parse(&started.authorize_url).unwrap();
        assert_eq!(authorize.host_str(), Some("login.microsoftonline.com"));
        assert!(
            authorize
                .query_pairs()
                .any(|(key, value)| key == "response_mode" && value == "form_post")
        );
        let claims = Claims {
            iss: CONSUMER_ISSUER.into(),
            aud: state.microsoft.as_ref().unwrap().client_id.clone(),
            sub: "verified-fixture-player".into(),
            nonce: flow.microsoft_nonce.clone(),
            exp: now() + 100,
            nbf: None,
        };
        let code = create_handoff(&state, &flow, &claims).await.unwrap();
        let wrong = exchange(
            State(state.clone()),
            Json(ExchangeRequest {
                code: code.clone(),
                verifier: secret().unwrap(),
            }),
        )
        .await;
        assert!(wrong.is_err());
        let (first, second) = tokio::join!(
            exchange(
                State(state.clone()),
                Json(ExchangeRequest {
                    code: code.clone(),
                    verifier: verifier.clone()
                })
            ),
            exchange(
                State(state.clone()),
                Json(ExchangeRequest {
                    code,
                    verifier: verifier.clone()
                })
            )
        );
        let signed_in = match (first, second) {
            (Ok(Json(session)), Err(_)) | (Err(_), Ok(Json(session))) => session,
            _ => panic!("exactly one exchange must issue a session"),
        };
        assert!(!signed_in.profile.setup_complete);
        assert_eq!(signed_in.profile.avatar, "violet");
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", signed_in.token)).unwrap(),
        );
        let Json(saved) = update_profile(
            State(state.clone()),
            headers.clone(),
            Json(ProfileUpdate {
                name: "  Alex  ".into(),
                avatar: "sky".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(saved.id, signed_in.profile.id);
        assert_eq!(saved.name, "Alex");
        assert_eq!(saved.avatar, "sky");
        assert!(saved.setup_complete);
        assert_eq!(
            logout(State(state.clone()), headers.clone()).await.unwrap(),
            StatusCode::NO_CONTENT
        );
        assert!(profile(State(state.clone()), headers).await.is_err());

        let renewed = create_handoff(&state, &flow, &claims).await.unwrap();
        let Json(session) = exchange(
            State(state),
            Json(ExchangeRequest {
                code: renewed,
                verifier,
            }),
        )
        .await
        .unwrap();
        assert_eq!(session.profile.id, saved.id);
        assert_eq!(session.profile.name, "Alex");
    }
}
