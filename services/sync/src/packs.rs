use std::net::SocketAddr;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use bezalel_pack_sync::{
    Account, Invite, InvitePreview, InviteRecord, JoinPack, NewInvite, NewPack, Pack, PackMember,
    Role, Target, sha256_hex, validate_sha256,
};
use serde::Deserialize;
use sqlx::{Executor, Postgres, Row, postgres::PgRow, types::Json as DbJson};
use uuid::Uuid;

use crate::{ApiError, ApiResult, AppState, authenticate, clean_name, now, secret};

fn pack_from(row: PgRow) -> ApiResult<Pack> {
    Ok(Pack {
        id: row.try_get("id")?,
        owner_id: row.try_get("owner_id")?,
        name: row.try_get("name")?,
        target: row.try_get::<DbJson<Target>, _>("target")?.0,
        server_address: row.try_get("server_address")?,
        role: Role::parse(row.try_get("role")?).map_err(|_| ApiError::internal())?,
        active_revision: row
            .try_get::<Option<i64>, _>("active_revision")?
            .map(|value| value as u64),
    })
}

pub(crate) async fn pack_for<'a>(
    database: impl Executor<'a, Database = Postgres>,
    pack: Uuid,
    account: Uuid,
) -> ApiResult<Pack> {
    let row = sqlx::query(
        "SELECT p.*, m.role FROM sync_packs p JOIN sync_members m ON m.pack_id = p.id
         WHERE p.id = $1 AND m.account_id = $2",
    )
    .bind(pack)
    .bind(account)
    .fetch_optional(database)
    .await?
    .ok_or_else(|| ApiError::missing("Pack membership not found"))?;
    pack_from(row)
}

pub(crate) fn require_owner(pack: &Pack) -> ApiResult<()> {
    if pack.role != Role::Owner {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

pub(crate) fn require_editor(pack: &Pack) -> ApiResult<()> {
    if pack.role == Role::Player {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

pub(crate) async fn list_packs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Pack>>> {
    let account = authenticate(&state, &headers).await?;
    let packs = sqlx::query(
        "SELECT p.*, m.role FROM sync_packs p JOIN sync_members m ON m.pack_id = p.id
         WHERE m.account_id = $1 ORDER BY p.name, p.id",
    )
    .bind(account.id)
    .fetch_all(&state.database)
    .await?;
    Ok(Json(
        packs
            .into_iter()
            .map(pack_from)
            .collect::<ApiResult<Vec<_>>>()?,
    ))
}

pub(crate) async fn create_pack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<NewPack>,
) -> ApiResult<Json<Pack>> {
    let account = authenticate(&state, &headers).await?;
    let name = clean_name(&input.name, 72)?;
    let server_address = input
        .server_address
        .as_deref()
        .map(validate_address)
        .transpose()?;
    input.target.validate().map_err(ApiError::bad)?;
    let pack = Uuid::new_v4();
    let mut transaction = state.database.begin().await?;
    sqlx::query("INSERT INTO sync_packs (id, owner_id, name, target, server_address) VALUES ($1, $2, $3, $4, $5)")
        .bind(pack).bind(account.id).bind(name).bind(DbJson(input.target)).bind(server_address)
        .execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO sync_members (pack_id, account_id, role) VALUES ($1, $2, 'owner')")
        .bind(pack)
        .bind(account.id)
        .execute(&mut *transaction)
        .await?;
    let output = pack_for(&mut *transaction, pack, account.id).await?;
    transaction.commit().await?;
    Ok(Json(output))
}

fn validate_address(address: &str) -> ApiResult<String> {
    let address = address.trim();
    if address.len() <= 253
        && (address
            .parse::<SocketAddr>()
            .is_ok_and(|value| value.port() != 0)
            || valid_hostname_address(address))
    {
        return Ok(address.into());
    }
    Err(ApiError::bad(
        "Enter a server hostname or IP address, with an optional port",
    ))
}

fn valid_hostname_address(address: &str) -> bool {
    let host = match address.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok_and(|port| port != 0) => host,
        Some(_) => return false,
        None => address,
    };
    host.trim_end_matches('.').split('.').all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes[0].is_ascii_alphanumeric()
            && bytes[bytes.len() - 1].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    })
}

pub(crate) async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
) -> ApiResult<Json<Vec<PackMember>>> {
    let account = authenticate(&state, &headers).await?;
    pack_for(&state.database, pack, account.id).await?;
    let rows = sqlx::query(
        "SELECT a.id, a.name, m.role FROM sync_members m JOIN sync_accounts a ON a.id = m.account_id
         WHERE m.pack_id = $1 ORDER BY a.name, a.id"
    ).bind(pack).fetch_all(&state.database).await?;
    let mut members = Vec::with_capacity(rows.len());
    for row in rows {
        members.push(PackMember {
            account: Account {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
            },
            role: Role::parse(row.try_get("role")?).map_err(|_| ApiError::internal())?,
        });
    }
    Ok(Json(members))
}

pub(crate) async fn remove_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pack, member)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let account = authenticate(&state, &headers).await?;
    let pack_data = pack_for(&state.database, pack, account.id).await?;
    if member == pack_data.owner_id || (member != account.id && pack_data.role != Role::Owner) {
        return Err(ApiError::forbidden());
    }
    sqlx::query("DELETE FROM sync_members WHERE pack_id = $1 AND account_id = $2")
        .bind(pack)
        .bind(member)
        .execute(&state.database)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
    Json(input): Json<NewInvite>,
) -> ApiResult<Json<Invite>> {
    let account = authenticate(&state, &headers).await?;
    require_owner(&pack_for(&state.database, pack, account.id).await?)?;
    if input.role == Role::Owner || !(300..=604_800).contains(&input.expires_in_seconds) {
        return Err(ApiError::bad(
            "Use a player or editor invite lasting five minutes to seven days",
        ));
    }
    let invite = Invite {
        id: Uuid::new_v4(),
        code: secret()?,
        expires_at: now() + i64::from(input.expires_in_seconds),
    };
    sqlx::query("INSERT INTO sync_invites (id, pack_id, code_sha256, role, expires_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(invite.id).bind(pack).bind(sha256_hex(invite.code.as_bytes())).bind(input.role.as_str()).bind(invite.expires_at)
        .execute(&state.database).await?;
    Ok(Json(invite))
}

pub(crate) async fn revoke_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pack, invite)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let account = authenticate(&state, &headers).await?;
    require_owner(&pack_for(&state.database, pack, account.id).await?)?;
    sqlx::query("UPDATE sync_invites SET revoked = true WHERE id = $1 AND pack_id = $2")
        .bind(invite)
        .bind(pack)
        .execute(&state.database)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_invites(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
) -> ApiResult<Json<Vec<InviteRecord>>> {
    let account = authenticate(&state, &headers).await?;
    require_owner(&pack_for(&state.database, pack, account.id).await?)?;
    let rows = sqlx::query(
        "SELECT id, role, expires_at, revoked FROM sync_invites
         WHERE pack_id = $1 AND expires_at > $2 ORDER BY expires_at DESC",
    )
    .bind(pack)
    .bind(now())
    .fetch_all(&state.database)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(InviteRecord {
                    id: row.try_get("id")?,
                    role: Role::parse(row.try_get("role")?).map_err(|_| ApiError::internal())?,
                    expires_at: row.try_get("expires_at")?,
                    revoked: row.try_get("revoked")?,
                })
            })
            .collect::<ApiResult<Vec<_>>>()?,
    ))
}

#[derive(Deserialize)]
pub(crate) struct PreviewRequest {
    code: String,
}

pub(crate) async fn preview_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PreviewRequest>,
) -> ApiResult<Json<InvitePreview>> {
    authenticate(&state, &headers).await?;
    let code = invite_digest(&input.code)?;
    let row = sqlx::query(
        "SELECT i.pack_id, i.role, i.expires_at, p.name AS pack_name, p.target, a.id AS owner_id, a.name AS owner_name
         FROM sync_invites i JOIN sync_packs p ON p.id = i.pack_id JOIN sync_accounts a ON a.id = p.owner_id
         WHERE i.code_sha256 = $1 AND NOT i.revoked AND i.expires_at > $2"
    ).bind(code).bind(now()).fetch_optional(&state.database).await?.ok_or_else(invite_missing)?;
    Ok(Json(InvitePreview {
        pack_id: row.try_get("pack_id")?,
        pack_name: row.try_get("pack_name")?,
        owner: Account {
            id: row.try_get("owner_id")?,
            name: row.try_get("owner_name")?,
        },
        target: row.try_get::<DbJson<Target>, _>("target")?.0,
        role: Role::parse(row.try_get("role")?).map_err(|_| ApiError::internal())?,
        expires_at: row.try_get("expires_at")?,
    }))
}

pub(crate) async fn join_pack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<JoinPack>,
) -> ApiResult<Json<Pack>> {
    let account = authenticate(&state, &headers).await?;
    let code = invite_digest(&input.code)?;
    let mut transaction = state.database.begin().await?;
    let row = sqlx::query(
        "SELECT i.pack_id, i.role, p.owner_id
         FROM sync_invites i JOIN sync_packs p ON p.id = i.pack_id
         WHERE i.code_sha256 = $1 AND NOT i.revoked AND i.expires_at > $2
         FOR UPDATE OF i",
    )
    .bind(code)
    .bind(now())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(invite_missing)?;
    let pack_id: Uuid = row.try_get("pack_id")?;
    let owner_id: Uuid = row.try_get("owner_id")?;
    if input.expected_owner_id != owner_id {
        return Err(ApiError::conflict(
            "This pack's owner changed. Review the invitation again.",
        ));
    }
    let role: String = row.try_get("role")?;
    sqlx::query(
        "INSERT INTO sync_members (pack_id, account_id, role) VALUES ($1, $2, $3)
         ON CONFLICT (pack_id, account_id) DO UPDATE SET role = 'editor'
         WHERE sync_members.role = 'player' AND EXCLUDED.role = 'editor'",
    )
    .bind(pack_id)
    .bind(account.id)
    .bind(role)
    .execute(&mut *transaction)
    .await?;
    let pack = pack_for(&mut *transaction, pack_id, account.id).await?;
    transaction.commit().await?;
    Ok(Json(pack))
}

fn invite_digest(code: &str) -> ApiResult<String> {
    let code = code.trim();
    validate_sha256(code).map_err(|_| invite_missing())?;
    Ok(sha256_hex(code.as_bytes()))
}

fn invite_missing() -> ApiError {
    ApiError::missing("This invite is invalid, expired, or revoked")
}
