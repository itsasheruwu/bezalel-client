use std::{
    collections::{BTreeMap, HashSet},
    io::Cursor,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bezalel_pack_sync::{
    Artifact, Candidate, CandidateRequest, FABRIC_API_PATH, MAX_ARTIFACT_BYTES, MAX_RELEASE_FILES,
    Pack, PublishRequest, ReleaseManifest, Role, SignedRelease, Submission, SubmitConfig,
    SubmitMod, fabric_api_artifact, sha256_hex, validate_artifact_path, validate_sha256,
    validate_source_changes,
};
use object_store::path::Path as ObjectPath;
use ring::signature::KeyPair;
use sqlx::{Executor, Postgres, Row, types::Json as DbJson};
use uuid::Uuid;

use crate::{
    ApiError, ApiResult, AppState, authenticate, now,
    packs::{pack_for, require_editor, require_owner},
};

fn object_path(pack: Uuid, digest: &str) -> ObjectPath {
    ObjectPath::from(format!("packs/{pack}/{digest}"))
}

fn notes(value: &str) -> ApiResult<String> {
    if value.len() > 4096
        || value
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(ApiError::bad(
            "Release notes are too long or contain unsupported characters",
        ));
    }
    Ok(value.trim().into())
}

pub(crate) async fn list_submissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
) -> ApiResult<Json<Vec<Submission>>> {
    let account = authenticate(&state, &headers).await?;
    let pack = pack_for(&state.database, pack, account.id).await?;
    require_editor(&pack)?;
    let rows = sqlx::query(
        "SELECT payload FROM sync_submissions
         WHERE pack_id = $1 AND ($2 OR author_id = $3)
         ORDER BY submitted_at DESC, id DESC LIMIT 24",
    )
    .bind(pack.id)
    .bind(pack.role == Role::Owner)
    .bind(account.id)
    .fetch_all(&state.database)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| Ok(row.try_get::<DbJson<Submission>, _>("payload")?.0))
            .collect::<ApiResult<Vec<_>>>()?,
    ))
}

pub(crate) async fn submit_mod(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
    Json(input): Json<SubmitMod>,
) -> ApiResult<Json<Submission>> {
    let account = authenticate(&state, &headers).await?;
    let pack_data = pack_for(&state.database, pack, account.id).await?;
    require_editor(&pack_data)?;
    let notes = notes(&input.notes)?;
    if !input.path.starts_with("mods/") || input.path.eq_ignore_ascii_case(FABRIC_API_PATH) {
        return Err(ApiError::bad("Mod submissions must be JAR files in mods/"));
    }
    validate_artifact_path(&input.path).map_err(ApiError::bad)?;
    validate_source_changes(&input.changes).map_err(ApiError::bad)?;
    if input.jar_base64.len() > MAX_ARTIFACT_BYTES.div_ceil(3) * 4 {
        return Err(ApiError::bad("The JAR exceeds the 16 MiB upload limit"));
    }
    let jar = STANDARD
        .decode(input.jar_base64)
        .map_err(|_| ApiError::bad("The JAR upload was not valid base64"))?;
    if jar.is_empty() || jar.len() > MAX_ARTIFACT_BYTES {
        return Err(ApiError::bad("The JAR exceeds the 16 MiB upload limit"));
    }
    validate_jar(&jar, &pack_data)?;
    let output = Submission {
        id: Uuid::new_v4(),
        author_id: account.id,
        artifact: Artifact {
            path: input.path,
            side: input.side,
            bytes: jar.len() as u64,
            sha256: sha256_hex(&jar),
        },
        changes: input.changes,
        notes,
        submitted_at: now(),
    };
    store_submission(&state, pack, &output, jar).await?;
    Ok(Json(output))
}

pub(crate) async fn submit_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
    Json(input): Json<SubmitConfig>,
) -> ApiResult<Json<Submission>> {
    let account = authenticate(&state, &headers).await?;
    require_owner(&pack_for(&state.database, pack, account.id).await?)?;
    if !input.path.starts_with("config/bezalel/")
        || input.text.is_empty()
        || input.text.len() > 256 * 1024
    {
        return Err(ApiError::bad(
            "Managed config must be a text file in config/bezalel/ and at most 256 KiB",
        ));
    }
    validate_artifact_path(&input.path).map_err(ApiError::bad)?;
    let bytes = input.text.into_bytes();
    let output = Submission {
        id: Uuid::new_v4(),
        author_id: account.id,
        artifact: Artifact {
            path: input.path,
            side: input.side,
            bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        },
        changes: Vec::new(),
        notes: notes(&input.notes)?,
        submitted_at: now(),
    };
    store_submission(&state, pack, &output, bytes).await?;
    Ok(Json(output))
}

async fn store_submission(
    state: &AppState,
    pack: Uuid,
    output: &Submission,
    bytes: Vec<u8>,
) -> ApiResult<()> {
    state
        .objects
        .put(
            &object_path(pack, &output.artifact.sha256),
            Bytes::from(bytes).into(),
        )
        .await?;
    // Recheck membership after the object upload. No candidate can refer to an object without a stored submission.
    let mut transaction = state.database.begin().await?;
    let membership = sqlx::query(
        "SELECT role FROM sync_members WHERE pack_id = $1 AND account_id = $2 FOR SHARE",
    )
    .bind(pack)
    .bind(output.author_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(ApiError::forbidden)?;
    if Role::parse(membership.try_get("role")?).map_err(ApiError::bad)? == Role::Player {
        return Err(ApiError::forbidden());
    }
    sqlx::query("INSERT INTO sync_submissions (id, pack_id, author_id, payload, submitted_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(output.id).bind(pack).bind(output.author_id).bind(DbJson(output)).bind(output.submitted_at)
        .execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(())
}

fn validate_jar(bytes: &[u8], pack: &Pack) -> ApiResult<()> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| ApiError::bad("This file is not a readable Java JAR"))?;
    let metadata = match pack.target.loader.as_str() {
        "fabric" => "fabric.mod.json",
        "forge" => "META-INF/mods.toml",
        "neoforge" => "META-INF/neoforge.mods.toml",
        _ => return Err(ApiError::bad("Unsupported mod loader")),
    };
    let found_metadata = archive.file_names().any(|name| name == metadata);
    let found_class = archive.file_names().any(|name| name.ends_with(".class"));
    if archive.len() > 10_000 || !found_metadata || !found_class {
        return Err(ApiError::bad(format!(
            "This JAR does not look like a compiled {} mod",
            pack.target.loader
        )));
    }
    Ok(())
}

async fn published<'a>(
    database: impl Executor<'a, Database = Postgres>,
    pack: Uuid,
    revision: u64,
) -> ApiResult<SignedRelease> {
    let revision =
        i64::try_from(revision).map_err(|_| ApiError::bad("Invalid release revision"))?;
    Ok(sqlx::query_scalar::<_, DbJson<SignedRelease>>(
        "SELECT signed FROM sync_releases WHERE pack_id = $1 AND revision = $2",
    )
    .bind(pack)
    .bind(revision)
    .fetch_optional(database)
    .await?
    .ok_or_else(|| ApiError::missing("Published revision not found"))?
    .0)
}

pub(crate) async fn list_candidates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
) -> ApiResult<Json<Vec<Candidate>>> {
    let account = authenticate(&state, &headers).await?;
    let pack = pack_for(&state.database, pack, account.id).await?;
    require_owner(&pack)?;
    let rows = sqlx::query_scalar::<_, DbJson<Candidate>>(
        "SELECT c.payload FROM sync_candidates c
         WHERE c.pack_id = $1 AND NOT EXISTS (SELECT 1 FROM sync_releases r WHERE r.candidate_id = c.id)
         ORDER BY c.created_at DESC, c.id DESC LIMIT 24",
    ).bind(pack.id).fetch_all(&state.database).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| row.0)
            .filter(|candidate| candidate.base_revision == pack.active_revision)
            .collect(),
    ))
}

pub(crate) async fn create_candidate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
    Json(input): Json<CandidateRequest>,
) -> ApiResult<Json<Candidate>> {
    let account = authenticate(&state, &headers).await?;
    let pack = pack_for(&state.database, pack, account.id).await?;
    require_owner(&pack)?;
    if input.base_revision != pack.active_revision {
        return Err(ApiError::conflict(
            "The active release changed. Refresh before preparing another update.",
        ));
    }
    if input.submission_ids.len() > MAX_RELEASE_FILES
        || input.remove_paths.len() > MAX_RELEASE_FILES
        || input.restore_revision.is_some()
            && (!input.submission_ids.is_empty() || !input.remove_paths.is_empty())
    {
        return Err(ApiError::bad(
            "Choose either a rollback or a set of release changes",
        ));
    }
    let revision = pack
        .active_revision
        .unwrap_or(0)
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(ApiError::internal)?;
    let previous_files = match pack.active_revision {
        Some(revision) => {
            published(&state.database, pack.id, revision)
                .await?
                .manifest
                .files
        }
        None => Vec::new(),
    };
    let mut files: BTreeMap<String, Artifact> = match input.restore_revision {
        Some(revision) => {
            published(&state.database, pack.id, revision)
                .await?
                .manifest
                .files
        }
        None => previous_files.clone(),
    }
    .into_iter()
    .map(|file| (file.path.to_ascii_lowercase(), file))
    .collect();
    for path in &input.remove_paths {
        validate_artifact_path(path).map_err(ApiError::bad)?;
        if path.eq_ignore_ascii_case(FABRIC_API_PATH) {
            return Err(ApiError::bad("Fabric API is pinned by the pack target"));
        }
        if files.remove(&path.to_ascii_lowercase()).is_none() {
            return Err(ApiError::bad(
                "The update removes a file outside its base revision",
            ));
        }
    }
    let mut added_paths = HashSet::new();
    for id in &input.submission_ids {
        let submission = sqlx::query_scalar::<_, DbJson<Submission>>(
            "SELECT payload FROM sync_submissions WHERE id = $1 AND pack_id = $2",
        )
        .bind(id)
        .bind(pack.id)
        .fetch_optional(&state.database)
        .await?
        .ok_or_else(|| ApiError::missing("A selected submission was not found in this pack"))?
        .0;
        let normalized = submission.artifact.path.to_ascii_lowercase();
        if !added_paths.insert(normalized.clone()) {
            return Err(ApiError::bad("Select only one submission per file"));
        }
        files.insert(normalized, submission.artifact);
    }
    if pack.target.loader == "fabric" && pack.target.minecraft_version == "1.21.1" {
        files.insert(FABRIC_API_PATH.into(), fabric_api_artifact());
    }
    let files: Vec<_> = files.into_values().collect();
    let server_paths: HashSet<&str> = files
        .iter()
        .filter(|file| file.side.server())
        .map(|file| file.path.as_str())
        .collect();
    let remove_server_paths = previous_files
        .into_iter()
        .filter(|file| file.side.server() && !server_paths.contains(file.path.as_str()))
        .map(|file| file.path)
        .collect();
    let manifest = ReleaseManifest {
        schema: 1,
        pack_id: pack.id,
        owner_id: pack.owner_id,
        revision,
        target: pack.target,
        files,
        notes: notes(&input.notes)?,
        created_at: now(),
    };
    let candidate = Candidate::sign(
        Uuid::new_v4(),
        input.base_revision,
        manifest,
        remove_server_paths,
        &state.signing_key,
    )
    .map_err(ApiError::bad)?;
    sqlx::query(
        "INSERT INTO sync_candidates (id, pack_id, payload, created_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(candidate.id)
    .bind(pack.id)
    .bind(DbJson(&candidate))
    .bind(candidate.release.manifest.created_at)
    .execute(&state.database)
    .await?;
    Ok(Json(candidate))
}

pub(crate) async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
    Json(input): Json<PublishRequest>,
) -> ApiResult<Json<SignedRelease>> {
    let account = authenticate(&state, &headers).await?;
    require_owner(&pack_for(&state.database, pack, account.id).await?)?;
    if !input.server_confirmed {
        return Err(ApiError::bad(
            "Back up, update, and test the server before publishing",
        ));
    }
    let mut transaction = state.database.begin().await?;
    sqlx::query("SELECT id FROM sync_packs WHERE id = $1 FOR UPDATE")
        .bind(pack)
        .fetch_one(&mut *transaction)
        .await?;
    let pack = pack_for(&mut *transaction, pack, account.id).await?;
    require_owner(&pack)?;
    if let Some(released) = sqlx::query_scalar::<_, DbJson<SignedRelease>>(
        "SELECT signed FROM sync_releases WHERE pack_id = $1 AND candidate_id = $2",
    )
    .bind(pack.id)
    .bind(input.candidate_id)
    .fetch_optional(&mut *transaction)
    .await?
    {
        transaction.commit().await?;
        return Ok(Json(released.0));
    }
    let candidate = sqlx::query_scalar::<_, DbJson<Candidate>>(
        "SELECT payload FROM sync_candidates WHERE id = $1 AND pack_id = $2",
    )
    .bind(input.candidate_id)
    .bind(pack.id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| ApiError::missing("Server candidate not found"))?
    .0;
    let public_key = state
        .signing_key
        .public_key()
        .as_ref()
        .try_into()
        .map_err(|_| ApiError::internal())?;
    candidate
        .verify(public_key)
        .map_err(|_| ApiError::internal())?;
    if candidate.base_revision != pack.active_revision
        || candidate.release.manifest.owner_id != pack.owner_id
        || candidate.release.manifest.target != pack.target
        || candidate.release.manifest.pack_id != pack.id
    {
        return Err(ApiError::conflict(
            "A newer release or owner is active. Prepare another server bundle.",
        ));
    }
    let release = SignedRelease::sign(candidate.release.manifest, &state.signing_key)
        .map_err(|_| ApiError::internal())?;
    let revision = release.manifest.revision as i64;
    sqlx::query("INSERT INTO sync_releases (pack_id, revision, candidate_id, signed) VALUES ($1, $2, $3, $4)")
        .bind(pack.id).bind(revision).bind(candidate.id).bind(DbJson(&release))
        .execute(&mut *transaction).await?;
    for entry in &release.manifest.files {
        if *entry == fabric_api_artifact() {
            continue;
        }
        sqlx::query("INSERT INTO sync_release_artifacts (pack_id, revision, sha256) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(pack.id).bind(revision).bind(&entry.sha256).execute(&mut *transaction).await?;
    }
    let updated = sqlx::query("UPDATE sync_packs SET active_revision = $2 WHERE id = $1 AND active_revision IS NOT DISTINCT FROM $3")
        .bind(pack.id).bind(revision).bind(candidate.base_revision.map(|value| value as i64))
        .execute(&mut *transaction).await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "A newer release is active. Prepare another server bundle.",
        ));
    }
    transaction.commit().await?;
    Ok(Json(release))
}

pub(crate) async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
) -> ApiResult<Json<Vec<SignedRelease>>> {
    let account = authenticate(&state, &headers).await?;
    pack_for(&state.database, pack, account.id).await?;
    let rows = sqlx::query_scalar::<_, DbJson<SignedRelease>>(
        "SELECT signed FROM sync_releases WHERE pack_id = $1 ORDER BY revision DESC LIMIT 50",
    )
    .bind(pack)
    .fetch_all(&state.database)
    .await?;
    Ok(Json(rows.into_iter().map(|row| row.0).collect()))
}

pub(crate) async fn active(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pack): Path<Uuid>,
) -> ApiResult<Json<Option<SignedRelease>>> {
    let account = authenticate(&state, &headers).await?;
    let pack = pack_for(&state.database, pack, account.id).await?;
    let active = match pack.active_revision {
        Some(revision) => Some(published(&state.database, pack.id, revision).await?),
        None => None,
    };
    Ok(Json(active))
}

pub(crate) async fn artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pack, digest)): Path<(Uuid, String)>,
) -> ApiResult<Response> {
    let account = authenticate(&state, &headers).await?;
    let pack = pack_for(&state.database, pack, account.id).await?;
    validate_sha256(&digest).map_err(ApiError::bad)?;
    let can_read: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM sync_release_artifacts WHERE pack_id = $1 AND sha256 = $2
            UNION ALL
            SELECT 1 FROM sync_submissions
            WHERE pack_id = $1 AND ( $3 OR ($4 AND author_id = $5) )
            AND payload->'artifact'->>'sha256' = $2
        )",
    )
    .bind(pack.id)
    .bind(&digest)
    .bind(pack.role == Role::Owner)
    .bind(pack.role == Role::Editor)
    .bind(account.id)
    .fetch_one(&state.database)
    .await?;
    if !can_read {
        return Err(ApiError::missing(
            "Artifact is not available to this member",
        ));
    }
    let object = state.objects.get(&object_path(pack.id, &digest)).await?;
    if object.meta.size == 0 || object.meta.size > MAX_ARTIFACT_BYTES as u64 {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "The stored artifact has an unexpected size",
        ));
    }
    let bytes = object.bytes().await?;
    if bytes.len() > MAX_ARTIFACT_BYTES || sha256_hex(&bytes) != digest {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "The stored artifact failed verification",
        ));
    }
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}
