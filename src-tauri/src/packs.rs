use crate::{
    commands::checked_jar,
    secrets,
    sharing_auth::{self, AccessCode, HostedApi, Profile},
    state::{self, err, Data, Pack, Result, State, Target, TARGETS},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use bezalel_pack_sync as sync;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Seek},
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::ipc::Channel;
use uuid::Uuid;

const SESSION_FILE: &str = "sharing-session.dpapi";

#[derive(Deserialize, Serialize)]
struct Session {
    account: sync::Account,
    token: String,
    url: String,
    key: String,
    #[serde(default)]
    profile: Option<Profile>,
    #[serde(default)]
    expires_at: Option<u64>,
}

fn session(state: &State) -> Result<Option<Session>> {
    secrets::read(&state.root, SESSION_FILE)?
        .map(|text| serde_json::from_str(&text).map_err(err))
        .transpose()
}

fn active_session(state: &State) -> Result<Session> {
    let current = session(state)?.ok_or("Connect Bezalel sharing in Settings first.")?;
    if current
        .expires_at
        .is_some_and(|expiry| expiry <= sharing_auth::now())
    {
        return Err("Your Bezalel session expired. Sign in again in Settings.".into());
    }
    Ok(current)
}

pub fn sharing_user(state: &State) -> Option<String> {
    active_session(state)
        .ok()
        .map(|session| session.account.name)
}

pub fn sharing_profile(state: &State) -> Option<Profile> {
    active_session(state)
        .ok()
        .and_then(|session| session.profile)
}

fn client(state: &State) -> Result<sync::SyncClient> {
    let session = active_session(state)?;
    sync::SyncClient::new(&session.url, &session.token, &session.key)
}

fn save_session(state: &State, current: &Session) -> Result<()> {
    secrets::write(
        &state.root,
        SESSION_FILE,
        &serde_json::to_string(current).map_err(err)?,
    )
}

fn connect_session(state: &State, current: &Session) -> Result<()> {
    let existing = session(state)?;
    let same_account = existing.is_some_and(|previous| {
        previous.url == current.url
            && previous.key == current.key
            && previous.account.id == current.account.id
    });
    state.update(|data| {
        if !same_account {
            data.packs.retain(|pack| pack.owner_id.is_none());
        }
        data.settings.sync_url.clone_from(&current.url);
        data.settings.sync_key.clone_from(&current.key);
        Ok(())
    })?;
    save_session(state, current)
}

fn sync_target(target: Target) -> sync::Target {
    sync::Target {
        loader: target.loader.into(),
        minecraft_version: target.minecraft_version.into(),
        loader_version: target.loader_version.into(),
    }
}

fn local_pack(remote: sync::Pack, previous: Option<&Pack>, trusted: bool) -> Pack {
    let target = TARGETS
        .iter()
        .copied()
        .find(|target| sync_target(*target) == remote.target);
    let trusted_owner_id = if trusted {
        Some(remote.owner_id)
    } else {
        previous
            .and_then(|pack| pack.trusted_owner_id)
            .filter(|owner| *owner == remote.owner_id)
    };
    Pack {
        id: remote.id,
        name: remote.name,
        target_id: target.map(|target| target.id.into()).unwrap_or_default(),
        address: remote.server_address.unwrap_or_default(),
        role: remote.role.as_str().into(),
        owner_id: Some(remote.owner_id),
        trusted_owner_id,
        active_revision: remote.active_revision,
        installed_revision: previous.and_then(|pack| pack.installed_revision),
    }
}

fn upsert(data: &mut Data, item: Pack) -> Pack {
    if let Some(old) = data.packs.iter_mut().find(|pack| pack.id == item.id) {
        *old = item.clone();
    } else {
        data.packs.push(item.clone());
    }
    item
}

fn server_address(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() > 255
        || value.starts_with('-')
        || value.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'-' | b':' | b'[' | b']' | b'_'))
        })
    {
        return Err(
            "Enter a Minecraft hostname or IP address, optionally followed by a port.".into(),
        );
    }
    Ok(value.into())
}

#[tauri::command]
pub async fn sync_connect(
    state: tauri::State<'_, State>,
    url: String,
    key: String,
    token: String,
) -> Result<()> {
    let _job = state.acquire("sharing-session".into())?;
    let url = url.trim().trim_end_matches('/').to_owned();
    let key = key.trim().to_owned();
    let token = token.trim().to_owned();
    let connection = sync::SyncClient::new(&url, &token, &key)?;
    let account = connection.me().await?;
    let current = Session {
        account,
        token,
        url,
        key,
        profile: None,
        expires_at: None,
    };
    connect_session(&state, &current)?;
    refresh(&state).await?;
    Ok(())
}

#[tauri::command]
pub async fn sync_sign_in(state: tauri::State<'_, State>, code: Option<String>) -> Result<Profile> {
    let _job = state.acquire("sharing-session".into())?;
    let (url, key) = sharing_auth::configured_service()?;
    let api = HostedApi::new(url)?;
    let sign_in = match code {
        Some(code) => api.redeem_code(code.trim()).await?,
        None => api.sign_in().await?,
    };
    let profile = sign_in.profile;
    let current = Session {
        account: sync::Account {
            id: profile.id,
            name: profile.name.clone(),
        },
        token: sign_in.token,
        url: url.trim_end_matches('/').into(),
        key: key.into(),
        profile: Some(profile.clone()),
        expires_at: Some(sign_in.expires_at),
    };
    connect_session(&state, &current)?;
    refresh(&state).await?;
    Ok(profile)
}

#[tauri::command]
pub async fn create_sync_access_code(state: tauri::State<'_, State>) -> Result<AccessCode> {
    let _job = state.acquire("sharing-session".into())?;
    let current = active_session(&state)?;
    if !current
        .profile
        .as_ref()
        .is_some_and(|profile| profile.can_invite)
    {
        return Err("Only the beta owner can create access codes.".into());
    }
    HostedApi::new(&current.url)?
        .create_access_code(&current.token)
        .await
}

#[tauri::command]
pub async fn save_sync_profile(
    state: tauri::State<'_, State>,
    name: String,
    avatar: String,
) -> Result<Profile> {
    let _job = state.acquire("sharing-session".into())?;
    let mut current = active_session(&state)?;
    if current.profile.is_none() {
        return Err("Sign in with Microsoft to edit a Bezalel profile.".into());
    }
    let profile = HostedApi::new(&current.url)?
        .save_profile(&current.token, name.trim(), &avatar)
        .await?;
    if profile.id != current.account.id {
        return Err("The sharing service returned a different account.".into());
    }
    current.account.name.clone_from(&profile.name);
    current.profile = Some(profile.clone());
    save_session(&state, &current)?;
    Ok(profile)
}

#[tauri::command]
pub async fn sync_disconnect(state: tauri::State<'_, State>) -> Result<()> {
    let _job = state.acquire("sharing-session".into())?;
    let revoke = match active_session(&state) {
        Ok(current) if current.profile.is_some() => match HostedApi::new(&current.url) {
            Ok(api) => api.logout(&current.token).await,
            Err(error) => Err(error),
        },
        _ => Ok(()),
    };
    secrets::remove(&state.root, SESSION_FILE)?;
    state.update(|data| {
        data.packs.retain(|pack| pack.owner_id.is_none());
        Ok(())
    })?;
    revoke.map_err(|_| "Signed out on this PC. The server could not confirm revocation.".into())
}

async fn refresh(state: &State) -> Result<Vec<Pack>> {
    let mut current = active_session(state)?;
    let remote = sync::SyncClient::new(&current.url, &current.token, &current.key)?
        .packs()
        .await?;
    if let Some(previous) = &current.profile {
        let profile = HostedApi::new(&current.url)?
            .profile(&current.token)
            .await?;
        if profile.id != current.account.id {
            return Err("The sharing service returned a different account.".into());
        }
        if &profile != previous {
            current.account.name.clone_from(&profile.name);
            current.profile = Some(profile);
            save_session(state, &current)?;
        }
    }
    state.update(|data| {
        let shared: Vec<Pack> = remote
            .into_iter()
            .map(|item| {
                let previous = data.packs.iter().find(|pack| pack.id == item.id);
                local_pack(item, previous, false)
            })
            .collect();
        data.packs.retain(|pack| pack.owner_id.is_none());
        data.packs.extend(
            shared
                .into_iter()
                .filter(|pack| !data.hidden_pack_ids.contains(&pack.id)),
        );
        Ok(data.packs.clone())
    })
}

#[tauri::command]
pub async fn refresh_packs(state: tauri::State<'_, State>) -> Result<Vec<Pack>> {
    let _job = state.acquire("sharing-session".into())?;
    refresh(&state).await
}

#[tauri::command]
pub async fn create_pack(
    state: tauri::State<'_, State>,
    name: String,
    target_id: String,
    address: String,
    shared: bool,
) -> Result<Pack> {
    let target = state::target(&target_id)?;
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("Enter a pack name between 1 and 64 characters.".into());
    }
    let address = server_address(&address)?;
    let pack = if shared {
        let remote = client(&state)?
            .create_pack(&sync::NewPack {
                name,
                target: sync_target(target),
                server_address: (!address.is_empty()).then_some(address),
            })
            .await?;
        local_pack(remote, None, true)
    } else {
        Pack {
            id: Uuid::new_v4(),
            name,
            target_id,
            address,
            owner_id: None,
            role: "owner".into(),
            trusted_owner_id: None,
            active_revision: None,
            installed_revision: None,
        }
    };
    state.update(|data| Ok(upsert(data, pack)))
}

#[tauri::command]
pub fn set_local_server_address(
    state: tauri::State<'_, State>,
    id: Uuid,
    address: String,
) -> Result<()> {
    let address = server_address(&address)?;
    let _job = state.acquire(format!("pack:{id}"))?;
    state.update(|data| {
        let mut pack = state::pack(data, id)?;
        if pack.owner_id.is_some() {
            return Err("Only local packs support editing the server address here.".into());
        }
        pack.address = address;
        upsert(data, pack);
        Ok(())
    })
}

#[tauri::command]
pub async fn invite_preview(
    state: tauri::State<'_, State>,
    code: String,
) -> Result<sync::InvitePreview> {
    client(&state)?.preview_invite(code.trim()).await
}

#[tauri::command]
pub async fn join_pack(
    state: tauri::State<'_, State>,
    code: String,
    owner_id: Uuid,
    trust: bool,
) -> Result<Pack> {
    if !trust {
        return Err("Review and confirm trust in the pack owner.".into());
    }
    let remote = client(&state)?
        .join(&sync::JoinPack {
            code,
            expected_owner_id: owner_id,
        })
        .await?;
    if remote.owner_id != owner_id {
        return Err("The pack owner changed. Review the invitation again.".into());
    }
    state.update(|data| {
        data.hidden_pack_ids.retain(|id| *id != remote.id);
        Ok(upsert(data, local_pack(remote, None, true)))
    })
}

#[tauri::command]
pub fn set_pack_trust(
    state: tauri::State<'_, State>,
    id: Uuid,
    trusted: bool,
    expected_owner: Option<Uuid>,
) -> Result<()> {
    let _job = state.acquire(format!("pack:{id}"))?;
    state.update(|data| {
        let pack = data
            .packs
            .iter_mut()
            .find(|pack| pack.id == id)
            .ok_or("Pack not found.")?;
        let owner = pack
            .owner_id
            .ok_or("This pack does not use shared releases.")?;
        if trusted && expected_owner != Some(owner) {
            return Err("The pack owner changed. Review trust again.".into());
        }
        pack.trusted_owner_id = trusted.then_some(owner);
        Ok(())
    })
}

pub async fn install(
    state: &State,
    item: &Pack,
    lock: &sync::PackLock,
    progress: &Channel<String>,
) -> Result<()> {
    let Some(owner_id) = item.owner_id else {
        return Ok(());
    };
    if item.trusted_owner_id != Some(owner_id) {
        return Err("Approve the pack owner in Settings → Downloads before playing.".into());
    }
    let client = client(state)?;
    let release = client
        .active(item.id)
        .await?
        .ok_or("The owner has not published a release for this pack yet.")?;
    let target = state::target(&item.target_id)?;
    if release.manifest.target != sync_target(target) {
        return Err("The release uses a different Minecraft target.".into());
    }
    let installed = client
        .install(
            &release,
            &sync::PackTrust {
                pack_id: item.id,
                owner_id,
            },
            lock,
            |current, total| {
                let label = if total == 0 {
                    "Checking installed files…".into()
                } else {
                    format!(
                        "Syncing {} · {}%",
                        item.name,
                        (current.saturating_mul(100) / total)
                    )
                };
                let _ = progress.send(label);
            },
        )
        .await?;
    state.update(|data| {
        let pack = data
            .packs
            .iter_mut()
            .find(|pack| pack.id == item.id)
            .ok_or("Pack membership changed.")?;
        pack.active_revision = Some(release.manifest.revision);
        pack.installed_revision = Some(installed.revision);
        if pack.trusted_owner_id != Some(owner_id) {
            return Err("Pack trust changed during the update.".into());
        }
        Ok(())
    })
}

#[tauri::command]
pub async fn sync_pack(
    state: tauri::State<'_, State>,
    id: Uuid,
    progress: Channel<String>,
) -> Result<()> {
    let _job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    let item = state::pack(&data, id)?;
    let dir = state.game_dir(&data, id);
    let lock = sync::PackLock::acquire(&dir)?;
    crate::minecraft::game_process::ensure_idle(&dir)?;
    install(&state, &item, &lock, &progress).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackDetails {
    active: Option<sync::SignedRelease>,
    history: Vec<sync::SignedRelease>,
    submissions: Vec<sync::Submission>,
    members: Vec<sync::PackMember>,
    candidates: Vec<sync::Candidate>,
    local_mods: Vec<String>,
}

#[tauri::command]
pub async fn pack_details(state: tauri::State<'_, State>, id: Uuid) -> Result<PackDetails> {
    let data = state.data()?;
    let item = state::pack(&data, id)?;
    if item.owner_id.is_none() {
        let directory = state.game_dir(&data, id).join("mods");
        let mut local_mods = Vec::new();
        if directory.exists() {
            state::ensure_directory(&directory)?;
            for entry in fs::read_dir(&directory).map_err(err)? {
                let entry = entry.map_err(err)?;
                let metadata = fs::symlink_metadata(entry.path()).map_err(err)?;
                if metadata.is_file() && !state::is_link(&metadata) {
                    if let Ok(name) = entry.file_name().into_string() {
                        if is_jar(Path::new(&name)) {
                            local_mods.push(name);
                        }
                    }
                }
            }
        }
        local_mods.sort();
        return Ok(PackDetails {
            active: None,
            history: Vec::new(),
            submissions: Vec::new(),
            members: Vec::new(),
            candidates: Vec::new(),
            local_mods,
        });
    }
    let client = client(&state)?;
    let (active, history, members) =
        tokio::try_join!(client.active(id), client.history(id), client.members(id))?;
    let (submissions, candidates) = if item.role == "owner" {
        tokio::try_join!(client.submissions(id), client.candidates(id))?
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(PackDetails {
        active,
        history,
        submissions,
        members,
        candidates,
        local_mods: Vec::new(),
    })
}

#[tauri::command]
pub async fn invite_create(
    state: tauri::State<'_, State>,
    id: Uuid,
    role: String,
) -> Result<sync::Invite> {
    client(&state)?
        .create_invite(
            id,
            &sync::NewInvite {
                role: sync::Role::parse(&role)?,
                expires_in_seconds: 86_400,
            },
        )
        .await
}

#[tauri::command]
pub async fn invite_revoke(
    state: tauri::State<'_, State>,
    id: Uuid,
    invite_id: Uuid,
) -> Result<()> {
    client(&state)?.revoke_invite(id, invite_id).await
}

#[tauri::command]
pub async fn member_remove(state: tauri::State<'_, State>, id: Uuid, user_id: Uuid) -> Result<()> {
    client(&state)?.remove_member(id, user_id).await
}

#[tauri::command]
pub async fn submit_mod(
    state: tauri::State<'_, State>,
    id: Uuid,
    draft_id: Uuid,
    side: sync::Side,
    notes: String,
) -> Result<sync::Submission> {
    let _job = state.acquire(format!("draft:{draft_id}"))?;
    let data = state.data()?;
    let item = state::pack(&data, id)?;
    let draft = state::draft(&data, draft_id)?;
    if draft.pack_id != Some(id) || item.target_id != draft.target_id {
        return Err("This draft targets a different pack.".into());
    }
    let build = draft.build.ok_or("Build and test the mod first.")?;
    if !(build.compile_passed && build.client_passed && build.server_passed) {
        return Err("Both smoke tests must pass before submitting a mod.".into());
    }
    let filename = build
        .jar_path
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or("Invalid mod filename.")?;
    let path = format!("mods/{filename}");
    sync::validate_artifact_path(&path)?;
    let bytes = checked_jar(&build)?;
    if bytes.len() > sync::MAX_ARTIFACT_BYTES {
        return Err("The JAR exceeds the shared-pack size limit.".into());
    }
    client(&state)?
        .submit(
            id,
            &sync::SubmitMod {
                path,
                side,
                notes,
                jar_base64: STANDARD.encode(bytes),
                changes: draft
                    .changes
                    .into_iter()
                    .map(|change| sync::SourceChange {
                        path: change.path,
                        content: change.content,
                    })
                    .collect(),
            },
        )
        .await
}

#[tauri::command]
pub async fn import_local_mod(
    state: tauri::State<'_, State>,
    id: Uuid,
    source: PathBuf,
) -> Result<String> {
    let _job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    if state::pack(&data, id)?.owner_id.is_some() {
        return Err("Mod files can only be imported into local packs.".into());
    }
    let dir = state.game_dir(&data, id);
    let _lock = sync::PackLock::acquire(&dir)?;
    crate::minecraft::game_process::ensure_idle(&dir)?;
    let mods = dir.join("mods");
    state::ensure_directory(&mods)?;
    tauri::async_runtime::spawn_blocking(move || copy_mod_file(&source, &mods))
        .await
        .map_err(err)?
}

fn is_jar(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
}

fn copy_mod_file(source: &Path, mods: &Path) -> Result<String> {
    if !source.is_absolute() || !is_jar(source) {
        return Err("Choose a Minecraft mod file ending in .jar.".into());
    }
    let name = source
        .file_name()
        .and_then(|part| part.to_str())
        .filter(|name| !name.contains(':'))
        .ok_or("Invalid mod filename.")?;
    let mut input =
        fs::File::open(source).map_err(|error| format!("Could not open {name}: {error}"))?;
    if !input.metadata().map_err(err)?.is_file()
        || !zip::ZipArchive::new(&mut input).is_ok_and(|archive| !archive.is_empty())
    {
        return Err("Choose a valid Minecraft mod JAR.".into());
    }
    input.rewind().map_err(err)?;

    let staged = mods.join(format!(".bezalel-import-{}.tmp", Uuid::new_v4()));
    let mut output = fs::File::create_new(&staged).map_err(err)?;
    let result = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    drop(output);
    let result =
        result.and_then(|_| crate::studio::platform::persist_new(&staged, &mods.join(name)));
    let _ = fs::remove_file(&staged);
    match result {
        Ok(()) => Ok(name.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(format!("{name} is already in this pack."))
        }
        Err(error) => Err(format!("Could not add {name}: {error}")),
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn imports_valid_jar_without_replacing_an_existing_mod() {
        let root = std::env::temp_dir().join(format!("bezalel-import-test-{}", Uuid::new_v4()));
        let mods = root.join("mods");
        fs::create_dir_all(&mods).unwrap();
        let jar = root.join("fixture.JAR");
        let mut zip = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();

        assert_eq!(copy_mod_file(&jar, &mods).unwrap(), "fixture.JAR");
        let installed = mods.join("fixture.JAR");
        assert_eq!(fs::read(&installed).unwrap(), fs::read(&jar).unwrap());
        fs::write(&installed, b"existing file").unwrap();
        assert!(copy_mod_file(&jar, &mods)
            .unwrap_err()
            .contains("already in this pack"));
        assert_eq!(fs::read(&installed).unwrap(), b"existing file");

        let invalid = root.join("invalid.jar");
        fs::write(&invalid, b"invalid JAR").unwrap();
        assert!(copy_mod_file(&invalid, &mods).is_err());
        assert_eq!(fs::read_dir(&mods).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}

#[tauri::command]
pub async fn install_local_mod(
    state: tauri::State<'_, State>,
    id: Uuid,
    draft_id: Uuid,
) -> Result<state::Draft> {
    let _draft_job = state.acquire(format!("draft:{draft_id}"))?;
    let _pack_job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    let pack = state::pack(&data, id)?;
    let draft = state::draft(&data, draft_id)?;
    if pack.owner_id.is_some() || pack.target_id != draft.target_id || draft.pack_id != Some(id) {
        return Err("This draft must target the selected local pack.".into());
    }
    let build = draft
        .build
        .as_ref()
        .ok_or("Build the mod before installing it.")?;
    if !build.compile_passed || !build.client_passed {
        return Err(
            "The compile and client startup tests must pass before installing a mod.".into(),
        );
    }
    let filename = build
        .jar_path
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or("Invalid mod filename.")?;
    sync::validate_artifact_path(&format!("mods/{filename}"))?;
    if sync::FABRIC_API_PATH.ends_with(&format!("/{filename}")) {
        return Err("This filename is reserved for Fabric API.".into());
    }
    let bytes = checked_jar(&build)?;
    let dir = state.game_dir(&data, id);
    let _lock = sync::PackLock::acquire(&dir)?;
    crate::minecraft::game_process::ensure_idle(&dir)?;
    let mods = dir.join("mods");
    state::ensure_directory(&mods)?;
    if state::target(&pack.target_id)?.loader == "fabric" {
        install_fabric_api(&dir).await?;
    }
    crate::mod_installation::install(&state, &draft, id, filename, &bytes, &build.sha256)
}

pub(crate) async fn install_fabric_api(game_dir: &Path) -> Result<()> {
    let artifact = sync::fabric_api_artifact();
    let destination = game_dir.join(&artifact.path);
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if !metadata.is_file() || state::is_link(&metadata) || metadata.len() != artifact.bytes {
            return Err("The local Fabric API file differs from the pinned dependency.".into());
        }
        let bytes = fs::read(&destination).map_err(err)?;
        if sync::sha256_hex(&bytes) == artifact.sha256 {
            return Ok(());
        }
        return Err("The local Fabric API file failed its integrity check.".into());
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(35))
        .build()
        .map_err(err)?;
    let mut response = http
        .get(sync::FABRIC_API_URL)
        .send()
        .await
        .map_err(err)?
        .error_for_status()
        .map_err(err)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(err)? {
        if bytes.len() as u64 + chunk.len() as u64 > artifact.bytes {
            return Err("Fabric API exceeded its expected size.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != artifact.bytes || sync::sha256_hex(&bytes) != artifact.sha256 {
        return Err("Fabric API failed publisher verification.".into());
    }
    crate::studio::write_atomic(&destination, &bytes)
}

#[tauri::command]
pub async fn create_candidate(
    state: tauri::State<'_, State>,
    id: Uuid,
    request: sync::CandidateRequest,
) -> Result<sync::Candidate> {
    client(&state)?.candidate(id, &request).await
}

#[tauri::command]
pub async fn export_candidate(
    state: tauri::State<'_, State>,
    id: Uuid,
    candidate_id: Uuid,
    destination: PathBuf,
) -> Result<()> {
    if !destination.is_absolute()
        || destination.extension().and_then(|part| part.to_str()) != Some("zip")
    {
        return Err("Select a ZIP file for the server bundle.".into());
    }
    let client = client(&state)?;
    let candidate = client
        .candidates(id)
        .await?
        .into_iter()
        .find(|item| item.id == candidate_id)
        .ok_or("Candidate not found.")?;
    client.export_server(&candidate, &destination).await
}

#[tauri::command]
pub async fn publish_candidate(
    state: tauri::State<'_, State>,
    id: Uuid,
    candidate_id: Uuid,
    server_confirmed: bool,
) -> Result<sync::SignedRelease> {
    let release = client(&state)?
        .publish(
            id,
            &sync::PublishRequest {
                candidate_id,
                server_confirmed,
            },
        )
        .await?;
    state.update(|data| {
        let pack = data
            .packs
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or("Pack not found.")?;
        pack.active_revision = Some(release.manifest.revision);
        Ok(())
    })?;
    Ok(release)
}

#[tauri::command]
pub fn delete_pack(state: tauri::State<'_, State>, id: Uuid) -> Result<()> {
    delete_pack_data(&state, id)
}

fn delete_pack_data(state: &State, id: Uuid) -> Result<()> {
    let _job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    state::pack(&data, id)?;
    let dir = state.game_dir(&data, id);
    if dir.exists() {
        let _lock = sync::PackLock::acquire(&dir)?;
        crate::minecraft::game_process::ensure_idle(&dir)?;
    }
    // Keep personal worlds and files intact. Removing a shared pack here does
    // not delete a server or another player's membership.
    state.update(|data| {
        data.packs.retain(|pack| pack.id != id);
        if !data.hidden_pack_ids.contains(&id) {
            data.hidden_pack_ids.push(id);
        }
        for draft in &mut data.drafts {
            if draft.pack_id == Some(id) {
                draft.pack_id = None;
            }
            if draft
                .installed_mod
                .as_ref()
                .is_some_and(|item| item.pack_id == id)
            {
                draft.installed_mod = None;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod deletion_tests {
    use super::*;
    #[test]
    fn deleting_pack_preserves_worlds_and_removes_library_entry() {
        let root = std::env::temp_dir().join(format!("bezalel-delete-{}", Uuid::new_v4()));
        let state = State::new(root.clone()).unwrap();
        let id = Uuid::new_v4();
        state
            .update(|data| {
                data.packs.push(Pack {
                    id,
                    name: "Fixture".into(),
                    target_id: "forge-1.21.1".into(),
                    address: String::new(),
                    owner_id: None,
                    role: "owner".into(),
                    trusted_owner_id: None,
                    active_revision: None,
                    installed_revision: None,
                });
                Ok(())
            })
            .unwrap();
        let dir = state.game_dir(&state.data().unwrap(), id);
        fs::create_dir_all(dir.join("saves")).unwrap();
        fs::write(dir.join("saves/world.dat"), b"personal world").unwrap();
        delete_pack_data(&state, id).unwrap();
        assert!(state.data().unwrap().packs.is_empty());
        assert!(state.data().unwrap().hidden_pack_ids.contains(&id));
        assert_eq!(
            fs::read(dir.join("saves/world.dat")).unwrap(),
            b"personal world"
        );
        assert!(delete_pack_data(&state, id).is_err());
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
}

#[tauri::command]
pub fn remove_local_mod(state: tauri::State<'_, State>, id: Uuid, filename: String) -> Result<()> {
    let _job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    if state::pack(&data, id)?.owner_id.is_some() {
        return Err("Shared mods must be removed through a published server update.".into());
    }
    let dir = state.game_dir(&data, id);
    let _lock = sync::PackLock::acquire(&dir)?;
    crate::minecraft::game_process::ensure_idle(&dir)?;
    archive_local_mod(&dir, &filename)?;
    state.update(|data| {
        for draft in &mut data.drafts {
            if draft
                .installed_mod
                .as_ref()
                .is_some_and(|item| item.pack_id == id && item.filename == filename)
            {
                draft.installed_mod = None;
            }
        }
        Ok(())
    })
}

fn archive_local_mod(dir: &Path, filename: &str) -> Result<()> {
    if filename.is_empty()
        || filename.contains(['/', '\\', ':'])
        || !is_jar(Path::new(filename))
        || Path::new(filename).file_name().and_then(|s| s.to_str()) != Some(filename)
    {
        return Err("Choose an installed mod filename.".into());
    }
    let mods = dir.join("mods");
    state::ensure_directory(&mods)?;
    let source = mods.join(filename);
    let meta = fs::symlink_metadata(&source).map_err(err)?;
    if !meta.is_file() || state::is_link(&meta) {
        return Err("Refusing to remove a redirected mod file.".into());
    }
    let archive = dir.join("removed-mods");
    state::ensure_directory(&archive)?;
    fs::rename(
        source,
        archive.join(format!("{}-{filename}", Uuid::new_v4())),
    )
    .map_err(err)
}

#[cfg(test)]
mod mod_removal_tests {
    use super::*;
    #[test]
    fn removes_only_selected_mod_and_rejects_traversal() {
        let root = std::env::temp_dir().join(format!("bezalel-mod-removal-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("mods")).unwrap();
        fs::write(root.join("mods/one.jar"), b"one").unwrap();
        fs::write(root.join("mods/two.jar"), b"two").unwrap();
        fs::write(root.join("world.dat"), b"world").unwrap();
        assert!(archive_local_mod(&root, "../world.dat").is_err());
        assert!(archive_local_mod(&root, "one.jar:stream").is_err());
        archive_local_mod(&root, "one.jar").unwrap();
        assert!(!root.join("mods/one.jar").exists());
        assert_eq!(fs::read(root.join("mods/two.jar")).unwrap(), b"two");
        assert_eq!(fs::read(root.join("world.dat")).unwrap(), b"world");
        let archived = fs::read_dir(root.join("removed-mods"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(archived).unwrap(), b"one");
        fs::remove_dir_all(root).unwrap();
    }
}
