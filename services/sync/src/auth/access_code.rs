use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use bezalel_pack_sync::sha256_hex;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{ApiError, ApiResult, AppState, authenticate, now};

use super::{ExchangeResponse, create_session};

pub(crate) async fn bootstrap_owner(database: &PgPool, digest: &str) -> Result<(), String> {
    let digest = digest.trim();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("BEZALEL_OWNER_ACCESS_CODE_SHA256 must be a hexadecimal SHA-256 digest".into());
    }
    let unavailable = || "Could not initialize the Bezalel beta owner".to_owned();
    let mut transaction = database.begin().await.map_err(|_| unavailable())?;
    let updated: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO sync_accounts (id, name, profile_complete, access_code_sha256, can_invite)
         VALUES ($1, 'Player', false, $2, true)
         ON CONFLICT (can_invite) WHERE can_invite DO UPDATE
         SET access_code_sha256 = EXCLUDED.access_code_sha256
         WHERE sync_accounts.access_code_sha256 IS DISTINCT FROM EXCLUDED.access_code_sha256
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(digest.to_ascii_lowercase())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| unavailable())?;
    if let Some(id) = updated {
        sqlx::query("DELETE FROM sync_sessions WHERE account_id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| unavailable())?;
    }
    transaction.commit().await.map_err(|_| unavailable())
}

fn normalized_code(code: &str) -> Option<String> {
    if code.len() > 96 {
        return None;
    }
    let mut normalized = String::with_capacity(32);
    for byte in code.bytes() {
        match byte {
            b' ' | b'-' => continue,
            _ if byte.is_ascii_hexdigit() && normalized.len() < 32 => {
                normalized.push(byte.to_ascii_lowercase() as char);
            }
            _ => return None,
        }
    }
    (normalized.len() == 32).then_some(normalized)
}

fn invalid_code() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "That Bezalel access code is invalid",
    )
}

async fn take_attempt(
    database: &PgPool,
    route: &'static str,
    time: i64,
    window: i64,
    limit: i32,
) -> ApiResult<()> {
    let admitted = sqlx::query_scalar::<_, i32>(
        "INSERT INTO sync_auth_limits (route, count, resets_at)
         VALUES ($1, 1, $2 + $3)
         ON CONFLICT (route) DO UPDATE SET
           count = CASE WHEN sync_auth_limits.resets_at <= $2 THEN 1 ELSE sync_auth_limits.count + 1 END,
           resets_at = CASE WHEN sync_auth_limits.resets_at <= $2 THEN $2 + $3 ELSE sync_auth_limits.resets_at END
         WHERE sync_auth_limits.resets_at <= $2 OR sync_auth_limits.count < $4
         RETURNING count",
    )
    .bind(route)
    .bind(time)
    .bind(window)
    .bind(limit)
    .fetch_optional(database)
    .await?;
    if admitted.is_none() {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many access code attempts. Try again later.",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct CodeRequest {
    code: String,
}

async fn redeem(
    State(state): State<AppState>,
    Json(input): Json<CodeRequest>,
) -> ApiResult<Json<ExchangeResponse>> {
    let time = now();
    take_attempt(&state.database, "redeem", time, 60, 120).await?;
    let code = normalized_code(&input.code).ok_or_else(invalid_code)?;
    let mut transaction = state.database.begin().await?;
    let id =
        sqlx::query_scalar("SELECT id FROM sync_accounts WHERE access_code_sha256 = $1 FOR SHARE")
            .bind(sha256_hex(code.as_bytes()))
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(invalid_code)?;
    let session = create_session(&mut transaction, id, time).await?;
    transaction.commit().await?;
    Ok(Json(session))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewCode {}

#[derive(Serialize)]
struct IssuedCode {
    code: String,
}

async fn invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(_): Json<NewCode>,
) -> ApiResult<Json<IssuedCode>> {
    let account = authenticate(&state, &headers).await?;
    let is_founder: bool = sqlx::query_scalar("SELECT can_invite FROM sync_accounts WHERE id = $1")
        .bind(account.id)
        .fetch_one(&state.database)
        .await?;
    if !is_founder {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only the beta owner can create access codes",
        ));
    }
    take_attempt(&state.database, "invite", now(), 3600, 20).await?;
    let mut entropy = [0; 16];
    getrandom::fill(&mut entropy).map_err(|_| ApiError::internal())?;
    let code = hex::encode(entropy);
    sqlx::query(
        "INSERT INTO sync_accounts (id, name, profile_complete, access_code_sha256)
         VALUES ($1, 'Player', false, $2)",
    )
    .bind(Uuid::new_v4())
    .bind(sha256_hex(code.as_bytes()))
    .execute(&state.database)
    .await?;
    let formatted = format!(
        "{}-{}-{}-{}",
        &code[..8],
        &code[8..16],
        &code[16..24],
        &code[24..]
    );
    Ok(Json(IssuedCode { code: formatted }))
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/code/redeem", post(redeem))
        .route("/v1/auth/code/invite", post(invite))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header};
    use ring::signature;
    use std::{env, sync::Arc};

    #[test]
    fn codes_normalize_without_accepting_non_hex_or_unicode() {
        let raw = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            normalized_code(" 01234567-89ABCDEF 01234567-89abcdef "),
            Some(raw.into())
        );
        assert!(normalized_code(raw).is_some());
        for bad in [
            "abc",
            "000000000000000000000000000000000",
            "é",
            "１２３４",
            "0123456789abcdef0123456789abcdex",
            "0123456789abcdef\t0123456789abcdef",
        ] {
            assert_eq!(normalized_code(bad), None, "{bad:?}");
        }
    }

    #[tokio::test]
    #[ignore = "requires a disposable PostgreSQL database via BEZALEL_AUTH_TEST_DATABASE_URL"]
    async fn access_codes_restore_profiles_without_granting_pack_or_founder_rights() {
        let database = PgPool::connect(
            &env::var("BEZALEL_AUTH_TEST_DATABASE_URL").expect("set disposable database URL"),
        )
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../schema.sql"))
            .execute(&database)
            .await
            .unwrap();
        let state = AppState {
            database,
            objects: Arc::new(object_store::memory::InMemory::new()),
            signing_key: Arc::new(
                signature::Ed25519KeyPair::from_seed_unchecked(&[5; 32]).unwrap(),
            ),
            admin_digest: None,
            microsoft: None,
        };
        let owner_code = "0a".repeat(16);
        let digest = sha256_hex(owner_code.as_bytes());
        bootstrap_owner(&state.database, &digest).await.unwrap();
        bootstrap_owner(&state.database, &digest.to_ascii_uppercase())
            .await
            .unwrap();
        let Json(owner) = redeem(
            State(state.clone()),
            Json(CodeRequest {
                code: owner_code.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(owner.profile.can_invite);
        let headers = |token: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
            headers
        };
        let owner_headers = headers(&owner.token);
        let Json(friend_code) = invite(
            State(state.clone()),
            owner_headers.clone(),
            Json(NewCode {}),
        )
        .await
        .unwrap();
        let Json(friend) = redeem(
            State(state.clone()),
            Json(CodeRequest {
                code: friend_code.code.to_ascii_uppercase().replace('-', " "),
            }),
        )
        .await
        .unwrap();
        assert_ne!(friend.profile.id, owner.profile.id);
        assert!(!friend.profile.can_invite);
        assert_eq!(
            invite(
                State(state.clone()),
                headers(&friend.token),
                Json(NewCode {})
            )
            .await
            .err()
            .unwrap()
            .status,
            StatusCode::FORBIDDEN
        );
        let memberships: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sync_members WHERE account_id = $1")
                .bind(friend.profile.id)
                .fetch_one(&state.database)
                .await
                .unwrap();
        assert_eq!(memberships, 0);
        let Json(restored) = redeem(
            State(state.clone()),
            Json(CodeRequest {
                code: friend_code.code,
            }),
        )
        .await
        .unwrap();
        assert_eq!(friend.profile.id, restored.profile.id);
        assert_ne!(friend.token, restored.token);
        bootstrap_owner(&state.database, &digest).await.unwrap();
        assert!(authenticate(&state, &owner_headers).await.is_ok());
        let replacement = "b5".repeat(16);
        bootstrap_owner(&state.database, &sha256_hex(replacement.as_bytes()))
            .await
            .unwrap();
        assert!(authenticate(&state, &owner_headers).await.is_err());
        let malformed = redeem(
            State(state.clone()),
            Json(CodeRequest {
                code: "g".repeat(32),
            }),
        )
        .await
        .err()
        .unwrap();
        let old = redeem(State(state.clone()), Json(CodeRequest { code: owner_code }))
            .await
            .err()
            .unwrap();
        assert_eq!(malformed.status, old.status);
        assert_eq!(malformed.message, old.message);
        let Json(replaced) = redeem(
            State(state.clone()),
            Json(CodeRequest { code: replacement }),
        )
        .await
        .unwrap();
        assert_eq!(replaced.profile.id, owner.profile.id);
        assert!(replaced.profile.can_invite);

        let time = now();
        take_attempt(&state.database, "fixture", time, 60, 1)
            .await
            .unwrap();
        assert_eq!(
            take_attempt(&state.database, "fixture", time, 60, 1)
                .await
                .unwrap_err()
                .status,
            StatusCode::TOO_MANY_REQUESTS
        );
        take_attempt(&state.database, "fixture", time + 60, 60, 1)
            .await
            .unwrap();
    }
}
