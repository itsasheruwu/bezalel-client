use crate::{
    builder::{self, BuildResult},
    builder_setup, draft_files, minecraft, packs, sharing_auth,
    state::{self, err, Data, Draft, Message, Result, Settings, State, TARGETS},
    studio::{self, StudioRequest},
};
use bezalel_pack_sync::PackLock;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tauri::{ipc::Channel, Manager};
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(flatten)]
    data: Data,
    targets: [state::Target; 3],
    chatgpt_connected: bool,
    microsoft_name: Option<String>,
    sync_user: Option<String>,
    sync_profile: Option<sharing_auth::Profile>,
    hosted_sharing_configured: bool,
    data_path: PathBuf,
}

#[tauri::command]
pub fn snapshot(state: tauri::State<'_, State>) -> Result<Snapshot> {
    Ok(Snapshot {
        data: state.data()?,
        targets: TARGETS,
        chatgpt_connected: studio::connected(&state.root),
        microsoft_name: minecraft::auth::connected(&state.root),
        sync_user: packs::sharing_user(&state),
        sync_profile: packs::sharing_profile(&state),
        hosted_sharing_configured: sharing_auth::configured_service().is_ok(),
        data_path: state.root.clone(),
    })
}

#[tauri::command]
pub fn save_settings(state: tauri::State<'_, State>, mut settings: Settings) -> Result<()> {
    if !["light", "dark", "system"].contains(&settings.theme.as_str())
        || !(2048..=16_384).contains(&settings.memory_mib)
        || ![90, 100, 110, 120, 130].contains(&settings.text_scale)
    {
        return Err("Invalid game or appearance setting.".into());
    }
    settings.microsoft_client_id = settings.microsoft_client_id.trim().to_owned();
    if !settings.microsoft_client_id.is_empty()
        && Uuid::parse_str(&settings.microsoft_client_id).is_err()
    {
        return Err("Enter the Microsoft application ID as a UUID.".into());
    }
    let _job = state.acquire("microsoft-auth".into())?;
    state.update(|data| {
        if settings.library_path != data.settings.library_path {
            if !data.packs.is_empty() {
                return Err("Choose a new library folder before adding packs.".into());
            }
            if let Some(path) = &settings.library_path {
                if !path.is_absolute() || !path.is_dir() {
                    return Err("Choose an existing local library folder.".into());
                }
            }
        }
        if settings.microsoft_client_id != data.settings.microsoft_client_id {
            minecraft::auth::disconnect(&state.root)?;
        }
        settings.sync_url.clone_from(&data.settings.sync_url);
        settings.sync_key.clone_from(&data.settings.sync_key);
        settings.onboarding_complete = data.settings.onboarding_complete;
        settings
            .model_family
            .clone_from(&data.settings.model_family);
        settings
            .reasoning_effort
            .clone_from(&data.settings.reasoning_effort);
        data.settings = settings;
        Ok(())
    })
}

#[tauri::command]
pub fn complete_onboarding(state: tauri::State<'_, State>) -> Result<()> {
    state.update(|data| {
        data.settings.onboarding_complete = true;
        Ok(())
    })
}

#[tauri::command]
pub fn create_draft(
    state: tauri::State<'_, State>,
    title: String,
    pack_id: Option<Uuid>,
    target_id: String,
    mode: String,
) -> Result<Draft> {
    let title = title.trim().to_owned();
    if title.is_empty() || title.chars().count() > 64 || title.chars().any(char::is_control) {
        return Err("Enter a mod name between 1 and 64 characters.".into());
    }
    if !matches!(mode.as_str(), "direct" | "guided") {
        return Err("Select a chat mode.".into());
    }
    state::target(&target_id)?;
    state.update(|data| {
        if let Some(id) = pack_id {
            if state::pack(data, id)?.target_id != target_id {
                return Err("Choose the same loader and version as your pack.".into());
            }
        }
        let draft = Draft {
            id: Uuid::new_v4(),
            title,
            pack_id,
            target_id,
            mode,
            thread_id: None,
            context_usage: None,
            installed_mod: None,
            mod_events: Vec::new(),
            idea: None,
            messages: Vec::new(),
            brief: None,
            changes: Vec::new(),
            build: None,
        };
        data.drafts.push(draft.clone());
        Ok(draft)
    })
}

#[tauri::command]
pub fn save_draft_idea(state: tauri::State<'_, State>, id: Uuid, idea: String) -> Result<()> {
    let _job = state.acquire(format!("draft:{id}"))?;
    let idea = idea.trim();
    if idea.len() > 48_000 {
        return Err("Enter a mod idea within the draft chat limit.".into());
    }
    state.update(|data| {
        state::update_draft(data, id, |draft| {
            draft.idea = (!idea.is_empty()).then(|| idea.to_owned());
        })
    })
}

#[tauri::command]
pub async fn studio_chat(
    state: tauri::State<'_, State>,
    id: Uuid,
    message: String,
    progress: Channel<String>,
) -> Result<Draft> {
    let _job = state.acquire(format!("draft:{id}"))?;
    if message.trim().is_empty() || message.len() > 48_000 {
        return Err("Enter a message within the draft chat limit.".into());
    }
    let draft = state::draft(&state.data()?, id)?;
    let target = state::target(&draft.target_id)?;
    let reply = studio::run(
        &state.root,
        StudioRequest {
            model_family: state.data()?.settings.model_family,
            reasoning_effort: state.data()?.settings.reasoning_effort,
            draft_id: id.to_string(),
            thread_id: draft.thread_id,
            loader: target.loader.into(),
            minecraft_version: target.minecraft_version.into(),
            mode: draft.mode,
            prompt: format!(
                "Mod name: {}\nPlayer message: {}",
                draft.title,
                message.trim()
            ),
        },
        |delta| {
            let _ = progress.send(delta);
        },
    )
    .await?;
    state.update(|data| {
        state::update_draft(data, id, |draft| {
            draft.thread_id = Some(reply.thread_id);
            draft.changes = reply.changes.clone();
            draft.build = None;
            draft.idea = None;
            draft.messages.push(Message {
                role: "user".into(),
                text: message,
                choices: Vec::new(),
            });
            let (text, choices) = if let Some(question) = reply.question {
                draft.brief = None;
                (question.text, question.choices)
            } else {
                (reply.message, Vec::new())
            };
            if !text.trim().is_empty() {
                draft.messages.push(Message {
                    role: "assistant".into(),
                    text,
                    choices,
                });
            }
            if let Some(brief) = reply.brief {
                draft.brief = Some(brief);
            } else {
                draft.brief = draft
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "user")
                    .map(|message| message.text.clone());
            }
            draft.clone()
        })
    })
}

#[tauri::command]
pub fn studio_cancel(id: Uuid) -> bool {
    studio::cancel(&id.to_string())
}

#[tauri::command]
pub fn open_minecraft_eula() -> Result<()> {
    studio::platform::open("https://www.minecraft.net/en-us/eula")
        .map_err(|_| "Could not open the Minecraft EULA in your browser.".into())
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum BuildModOutcome {
    Complete { draft: Box<Draft> },
    ConsentRequired,
}

#[tauri::command]
pub async fn build_mod(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
    id: Uuid,
    brief: String,
    accept_minecraft_eula: Option<bool>,
    progress: Channel<String>,
) -> Result<BuildModOutcome> {
    let resources = app.path().resource_dir().map_err(err)?;
    build_draft(
        &state,
        &resources,
        id,
        brief,
        accept_minecraft_eula,
        |message| {
            let _ = progress.send(message);
        },
    )
    .await
}

#[tauri::command]
pub async fn prepare_mod_tools(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
    target_id: String,
    accept_minecraft_eula: Option<bool>,
    progress: Channel<String>,
) -> Result<builder_setup::Outcome> {
    let target = state::target(&target_id)?;
    let resources = app.path().resource_dir().map_err(err)?;
    ensure_mod_tools(
        &state,
        &resources,
        target,
        accept_minecraft_eula == Some(true),
        &|text| {
            let _ = progress.send(text);
        },
    )
    .await
}

async fn ensure_mod_tools(
    state: &State,
    resources: &Path,
    target: state::Target,
    accepted_now: bool,
    progress: &(impl Fn(String) + Send + Sync),
) -> Result<builder_setup::Outcome> {
    // Consent is checked before probing, installing or repairing any tools.
    if accepted_now {
        state.update(|data| {
            data.builder_eula_accepted = true;
            Ok(())
        })?;
    }
    if !state.data()?.builder_eula_accepted {
        return Ok(builder_setup::Outcome::ConsentRequired);
    }
    let _setup = builder_setup::gate().lock().await;
    if builder::check_environment(target.loader, target.minecraft_version)
        .await
        .is_ok()
    {
        return Ok(builder_setup::Outcome::Ready);
    }
    progress("Preparing your mod tools. You can keep working in chat...".into());
    let outcome = builder_setup::prepare(
        resources,
        &state.root.join("builder-setup-managed"),
        target.loader,
        target.minecraft_version,
        true,
        progress,
    )
    .await?;
    if outcome == builder_setup::Outcome::Ready {
        builder::check_environment(target.loader, target.minecraft_version).await?;
    }
    Ok(outcome)
}

async fn build_draft(
    state: &State,
    resources: &Path,
    id: Uuid,
    brief: String,
    accept_minecraft_eula: Option<bool>,
    progress: impl Fn(String) + Send + Sync,
) -> Result<BuildModOutcome> {
    validate_mod_brief(&brief)?;
    let _job = state.acquire(format!("draft:{id}"))?;
    let draft = state::draft(&state.data()?, id)?;
    let target = state::target(&draft.target_id)?;
    if ensure_mod_tools(
        state,
        resources,
        target,
        accept_minecraft_eula == Some(true),
        &progress,
    )
    .await?
        == builder_setup::Outcome::ConsentRequired
    {
        return Ok(BuildModOutcome::ConsentRequired);
    }
    let source = state
        .root
        .join("drafts")
        .join(id.to_string())
        .join("src/main");
    if !source.is_dir() {
        generate_draft_source(state, draft, target, brief, &progress).await?;
    } else {
        progress("Building your current mod sources...".into());
    }
    let build = builder::build(
        &state.root,
        resources,
        &id.to_string(),
        target.loader,
        target.minecraft_version,
        progress,
    )
    .await?;
    let draft = state.update(|data| {
        state::update_draft(data, id, |draft| {
            draft.build = Some(build);
            draft.clone()
        })
    })?;
    Ok(BuildModOutcome::Complete {
        draft: Box::new(draft),
    })
}

#[tauri::command]
pub async fn generate_mod_source(
    state: tauri::State<'_, State>,
    id: Uuid,
    brief: String,
    progress: Channel<String>,
) -> Result<Draft> {
    let _job = state.acquire(format!("draft:{id}"))?;
    let draft = state::draft(&state.data()?, id)?;
    let target = state::target(&draft.target_id)?;
    generate_draft_source(&state, draft, target, brief, &|message| {
        let _ = progress.send(message);
    })
    .await
}

fn validate_mod_brief(brief: &str) -> Result<()> {
    if brief.trim().is_empty() || brief.len() > 48_000 {
        return Err("Enter a mod brief within the build limit.".into());
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod live_builder_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Uses an explicitly selected signed-in profile and updates its draft"]
    async fn builds_existing_draft_in_background() {
        let root =
            PathBuf::from(std::env::var_os("BEZALEL_TEST_DATA").expect("Set BEZALEL_TEST_DATA"));
        let resources = PathBuf::from(
            std::env::var_os("BEZALEL_TEST_RESOURCES").expect("Set BEZALEL_TEST_RESOURCES"),
        );
        let id =
            Uuid::parse_str(&std::env::var("BEZALEL_TEST_DRAFT").expect("Set BEZALEL_TEST_DRAFT"))
                .expect("Use a draft UUID");
        assert!(
            root.join("bezalel.sqlite").is_file(),
            "Use an existing profile"
        );
        let state = State::new(root).expect("Open the selected test profile");
        let draft =
            state::draft(&state.data().expect("Read test data"), id).expect("Find test draft");
        let outcome = build_draft(
            &state,
            &resources,
            id,
            draft.brief.expect("Use a draft with a saved brief"),
            None,
            |message| println!("{message}"),
        )
        .await
        .expect("Build the selected draft");
        let BuildModOutcome::Complete { draft } = outcome else {
            panic!("The existing builder requested setup consent");
        };
        let build = draft.build.expect("The build must produce a JAR");
        assert!(
            build.compile_passed && build.client_passed && build.server_passed,
            "{:?}",
            build.diagnostics
        );
        checked_jar(&build).expect("Verify the built JAR");
        println!("Verified JAR SHA-256: {}", build.sha256);
    }
}

async fn generate_draft_source(
    state: &State,
    draft: Draft,
    target: state::Target,
    brief: String,
    progress: &(impl Fn(String) + Sync),
) -> Result<Draft> {
    validate_mod_brief(&brief)?;
    let id = draft.id;
    progress("Codex is preparing Java source…".into());
    let reply = studio::run(
        &state.root,
        StudioRequest {
            model_family: state.data()?.settings.model_family,
            reasoning_effort: state.data()?.settings.reasoning_effort,
            draft_id: id.to_string(),
            thread_id: draft.thread_id,
            loader: target.loader.into(),
            minecraft_version: target.minecraft_version.into(),
            mode: "build".into(),
            prompt: format!(
                "Mod name: {}\nApproved brief: {}",
                draft.title,
                brief.trim()
            ),
        },
        |_| {},
    )
    .await?;
    state.update(|data| {
        state::update_draft(data, id, |draft| {
            draft.thread_id = Some(reply.thread_id);
            draft.brief = Some(brief);
            draft.build = None;
        })
    })?;
    // Codex edits the real workspace directly.

    state.update(|data| {
        state::update_draft(data, id, |draft| {
            draft.changes = reply.changes;
            if !reply.message.is_empty() {
                draft.messages.push(Message {
                    role: "assistant".into(),
                    text: reply.message,
                    choices: Vec::new(),
                });
            }
            draft.clone()
        })
    })
}

pub fn checked_jar(build: &BuildResult) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(&build.jar_path).map_err(err)?;
    if !metadata.is_file()
        || state::is_link(&metadata)
        || metadata.len() != build.size as u64
        || build.size > 64 * 1024 * 1024
    {
        return Err("The built JAR has changed. Rebuild it first.".into());
    }
    let bytes = fs::read(&build.jar_path).map_err(err)?;
    if format!("{:x}", Sha256::digest(&bytes)) != build.sha256 {
        return Err("The built JAR failed its integrity check. Rebuild it first.".into());
    }
    Ok(bytes)
}

fn export_file(destination: PathBuf, extension: &str, bytes: &[u8]) -> Result<()> {
    if !destination.is_absolute()
        || destination.extension().and_then(|part| part.to_str()) != Some(extension)
    {
        return Err(format!("Select a .{extension} file."));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "Choose a new filename for the export.".to_owned()
            } else {
                err(error)
            }
        })?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(destination);
        return Err(err(error));
    }
    Ok(())
}

#[tauri::command]
pub fn export_jar(state: tauri::State<'_, State>, id: Uuid, destination: PathBuf) -> Result<()> {
    let _job = state.acquire(format!("draft:{id}"))?;
    let build = state::draft(&state.data()?, id)?
        .build
        .ok_or("Build the mod before exporting it.")?;
    export_file(destination, "jar", &checked_jar(&build)?)
}

#[tauri::command]
pub async fn connect_chatgpt(state: tauri::State<'_, State>) -> Result<()> {
    let _job = state.acquire("chatgpt-auth".into())?;
    studio::connect(&state.root).await
}

#[tauri::command]
pub async fn disconnect_chatgpt(state: tauri::State<'_, State>) -> Result<()> {
    let _job = state.acquire("chatgpt-auth".into())?;
    studio::disconnect(&state.root).await
}

#[tauri::command]
pub async fn connect_microsoft(state: tauri::State<'_, State>) -> Result<String> {
    let _job = state.acquire("microsoft-auth".into())?;
    minecraft::auth::connect(&state.root, &state.data()?.settings.microsoft_client_id).await
}

#[tauri::command]
pub fn disconnect_microsoft(state: tauri::State<'_, State>) -> Result<()> {
    let _job = state.acquire("microsoft-auth".into())?;
    minecraft::auth::disconnect(&state.root)
}

#[tauri::command]
pub async fn play(
    state: tauri::State<'_, State>,
    id: Uuid,
    progress: Channel<String>,
) -> Result<()> {
    let _pack_job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    let item = state::pack(&data, id)?;
    let target = state::target(&item.target_id)?;
    let game_dir = state.game_dir(&data, id);
    let lock = PackLock::acquire(&game_dir)?;
    minecraft::game_process::ensure_idle(&game_dir)?;
    let _job = state.acquire("microsoft-auth".into())?;
    let profile = minecraft::auth::refresh(&state.root, &data.settings.microsoft_client_id).await?;
    packs::install(&state, &item, &lock, &progress).await?;
    let _ = progress.send("Preparing Minecraft…".into());
    let mut game = minecraft::launch::launch(
        &state.root,
        &game_dir,
        target,
        &profile,
        data.settings.memory_mib,
        &item.address,
        |value| {
            let _ = progress.send(value);
        },
    )
    .await?;
    if let Err(error) = game
        .id()
        .ok_or("Minecraft exited before launch completed.".into())
        .and_then(|pid| minecraft::game_process::record(&game_dir, pid))
    {
        let _ = game.kill().await;
        return Err(error);
    }
    tauri::async_runtime::spawn(async move {
        let _ = game.wait().await;
        drop(lock);
    });
    Ok(())
}

#[tauri::command]
pub fn open_instance(state: tauri::State<'_, State>, id: Uuid) -> Result<()> {
    let data = state.data()?;
    state::pack(&data, id)?;
    let dir = state.game_dir(&data, id);
    state::ensure_directory(&dir)?;
    open_folder(&dir)
}

fn open_folder(dir: &std::path::Path) -> Result<()> {
    studio::platform::open(
        dir.to_str()
            .ok_or("The game folder path cannot be opened.")?,
    )
    .map_err(|_| "Could not open the game folder.".into())
}

#[tauri::command]
pub async fn prepare_handoff(
    state: tauri::State<'_, State>,
    id: Uuid,
    progress: Channel<String>,
) -> Result<LauncherHandoff> {
    let _job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    let item = state::pack(&data, id)?;
    let target = state::target(&item.target_id)?;
    let dir = state.game_dir(&data, id);
    let lock = PackLock::acquire(&dir)?;
    minecraft::game_process::ensure_idle(&dir)?;
    let (game_directory, revision) = if item.owner_id.is_some() {
        packs::install(&state, &item, &lock, &progress).await?;
        let revision = lock
            .installed()?
            .ok_or("No pack release has been installed.")?
            .revision;
        (
            minecraft::official::shared_snapshot(&dir, id, revision)?,
            Some(revision),
        )
    } else {
        (dir, None)
    };
    let version_id = minecraft::official::version_id(target)?;
    minecraft::official::select_pack(id, &item.name, &game_directory, &version_id)?;
    minecraft::official::open()?;
    Ok(LauncherHandoff {
        game_directory,
        version_id,
        revision,
        address: item.address,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherHandoff {
    game_directory: PathBuf,
    version_id: String,
    revision: Option<u64>,
    address: String,
}

#[tauri::command]
pub async fn open_minecraft_launcher() -> Result<()> {
    minecraft::official::open()
}

#[tauri::command]
pub fn open_official_loader(state: tauri::State<'_, State>, id: Uuid) -> Result<()> {
    let item = state::pack(&state.data()?, id)?;
    let url = match state::target(&item.target_id)?.loader {
        "fabric" => "https://docs.fabricmc.net/players/installing-fabric/windows",
        "forge" => "https://files.minecraftforge.net/net/minecraftforge/forge/index_1.21.1.html",
        "neoforge" => "https://docs.neoforged.net/user/docs/client/",
        _ => return Err("This Minecraft loader is unavailable.".into()),
    };
    studio::platform::open(url).map_err(|_| "Could not open the loader's website.".into())
}

#[tauri::command]
pub fn export_diagnostics(state: tauri::State<'_, State>, destination: PathBuf) -> Result<()> {
    let data = state.data()?;
    let payload = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "targets": TARGETS,
        "counts": { "packs": data.packs.len(), "drafts": data.drafts.len() },
        "settings": { "memoryMib": data.settings.memory_mib, "autoSync": data.settings.auto_sync },
        "connections": { "minecraft": minecraft::auth::connected(&state.root).is_some(),
            "chatgpt": studio::connected(&state.root), "sharing": packs::sharing_user(&state).is_some() }
    });
    export_file(
        destination,
        "json",
        &serde_json::to_vec_pretty(&payload).map_err(err)?,
    )
}

#[tauri::command]
pub fn mod_sources(state: tauri::State<'_, State>, id: Uuid) -> Result<Vec<studio::StudioChange>> {
    state::draft(&state.data()?, id)?;
    studio::source_changes(&state.root, &id.to_string())
}

#[tauri::command]
pub fn save_mod_source(
    state: tauri::State<'_, State>,
    id: Uuid,
    path: String,
    content: String,
) -> Result<()> {
    save_mod_source_data(&state, id, path, content)
}

fn save_mod_source_data(state: &State, id: Uuid, path: String, content: String) -> Result<()> {
    let _job = state.acquire(format!("draft:{id}"))?;
    state::draft(&state.data()?, id)?;
    let change = studio::StudioChange {
        path,
        content: Some(content),
    };
    draft_files::apply(&state.root, id, &[change])?;
    let changes = studio::source_changes(&state.root, &id.to_string())?;
    state.update(|data| {
        state::update_draft(data, id, |draft| {
            draft.build = None;
            draft.changes = changes;
        })
    })
}

#[tauri::command]
pub fn open_mod_workspace(state: tauri::State<'_, State>, id: Uuid) -> Result<()> {
    state::draft(&state.data()?, id)?;
    let dir = state.root.join("drafts").join(id.to_string());
    state::ensure_directory(&dir)?;
    open_folder(&dir)
}

#[tauri::command]
pub fn studio_respond(request_id: String, result: serde_json::Value) -> Result<()> {
    studio::respond(&request_id, result)
}

#[cfg(test)]
mod source_edit_tests {
    use super::*;
    #[test]
    fn source_edits_and_conversation_deletion_preserve_files() {
        let root = std::env::temp_dir().join(format!("bezalel-source-edit-{}", Uuid::new_v4()));
        let state = State::new(root.clone()).unwrap();
        let id = Uuid::new_v4();
        state
            .update(|data| {
                data.drafts.push(Draft {
                    id,
                    pack_id: None,
                    title: "Fixture".into(),
                    target_id: "forge-1.21.1".into(),
                    mode: "direct".into(),
                    thread_id: None,
                    context_usage: None,
                    installed_mod: None,
                    mod_events: Vec::new(),
                    idea: None,
                    messages: vec![],
                    brief: None,
                    changes: vec![],
                    build: None,
                });
                Ok(())
            })
            .unwrap();
        let path = "src/main/java/example/Mod.java";
        save_mod_source_data(&state, id, path.into(), "class Mod {}".into()).unwrap();
        let resource = root
            .join("drafts")
            .join(id.to_string())
            .join("src/main/resources/texture.png");
        fs::create_dir_all(resource.parent().unwrap()).unwrap();
        fs::write(&resource, b"binary fixture").unwrap();
        save_mod_source_data(
            &state,
            id,
            path.into(),
            "class Mod { int edited = 1; }".into(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("drafts").join(id.to_string()).join(path)).unwrap(),
            "class Mod { int edited = 1; }"
        );
        assert_eq!(fs::read(resource).unwrap(), b"binary fixture");
        assert!(state::draft(&state.data().unwrap(), id)
            .unwrap()
            .changes
            .iter()
            .any(|change| change.path == path
                && change.content.as_deref() == Some("class Mod { int edited = 1; }")));
        assert!(save_mod_source_data(&state, id, "../escape.java".into(), "bad".into()).is_err());
        assert!(!root.join("escape.java").exists());
        let installed = root.join("instances/fixture/mods/fixture.jar");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::write(&installed, b"installed mod").unwrap();
        let job = state.acquire(format!("draft:{id}")).unwrap();
        assert!(delete_draft_data(&state, id).is_err());
        drop(job);
        delete_draft_data(&state, id).unwrap();
        assert!(state.data().unwrap().drafts.is_empty());
        assert!(!root.join("drafts").join(id.to_string()).exists());
        let archived = fs::read_dir(root.join("deleted-projects"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(archived.join(path)).unwrap(),
            "class Mod { int edited = 1; }"
        );
        assert_eq!(fs::read(installed).unwrap(), b"installed mod");

        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
}

#[tauri::command]
pub fn delete_draft(state: tauri::State<'_, State>, id: Uuid) -> Result<()> {
    delete_draft_data(&state, id)
}

fn delete_draft_data(state: &State, id: Uuid) -> Result<()> {
    let _job = state.acquire(format!("draft:{id}"))?;
    state::draft(&state.data()?, id)?;
    let drafts = state.root.join("drafts");
    state::ensure_directory(&drafts)?;
    let project = drafts.join(id.to_string());
    let archive = state.root.join("deleted-projects");
    state::ensure_directory(&archive)?;
    let destination = archive.join(format!("{id}-{}", Uuid::new_v4()));
    let moved = match fs::symlink_metadata(&project) {
        Ok(meta) => {
            if !meta.is_dir() || state::is_link(&meta) {
                return Err("Refusing to archive a redirected project directory.".into());
            }
            fs::rename(&project, &destination).map_err(err)?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(err(error)),
    };
    let result = state.update(|data| {
        data.drafts.retain(|draft| draft.id != id);
        Ok(())
    });
    if result.is_err() && moved {
        fs::rename(destination, project).map_err(err)?;
    }
    result
}

#[cfg(test)]
mod mod_tools_tests {
    use super::*;
    #[tokio::test]
    async fn first_time_build_requests_consent_before_touching_existing_tools() {
        let root = std::env::temp_dir().join(format!("bezalel-consent-{}", Uuid::new_v4()));
        let state = State::new(root.clone()).unwrap();
        let target = state::target("forge-1.21.1").unwrap();
        let outcome = ensure_mod_tools(
            &state,
            Path::new("missing-resources"),
            target,
            false,
            &|_| panic!("No setup before consent"),
        )
        .await
        .unwrap();
        assert_eq!(outcome, builder_setup::Outcome::ConsentRequired);
        assert!(!state.data().unwrap().builder_eula_accepted);
        assert!(!root.join("builder-setup-managed").exists());
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
}

#[tauri::command]
pub async fn agent_models(
    state: tauri::State<'_, State>,
) -> Result<Vec<crate::agent_models::AgentModel>> {
    studio::models(&state.root).await
}

#[tauri::command]
pub fn save_agent_preferences(
    state: tauri::State<'_, State>,
    model_family: String,
    reasoning_effort: String,
) -> Result<()> {
    if !crate::agent_models::FAMILIES.contains(&model_family.as_str())
        || !crate::agent_models::EFFORTS.contains(&reasoning_effort.as_str())
    {
        return Err("Choose a supported model and reasoning level.".into());
    }
    state.update(|data| {
        data.settings.model_family = model_family;
        data.settings.reasoning_effort = reasoning_effort;
        Ok(())
    })
}

#[tauri::command]
pub fn apply_app_theme(app: tauri::AppHandle, theme: String) -> Result<()> {
    crate::theme::apply(&app, &theme)
}

#[derive(Serialize)]
pub struct StudioAttachment {
    name: String,
    path: String,
}

#[tauri::command]
pub fn attach_studio_file(
    state: tauri::State<'_, State>,
    id: Uuid,
    source: PathBuf,
) -> Result<StudioAttachment> {
    let _job = state.acquire(format!("draft:{id}"))?;
    state::draft(&state.data()?, id)?;
    if !source.is_absolute() {
        return Err("Select a local file.".into());
    }
    let meta = fs::symlink_metadata(&source).map_err(err)?;
    if !meta.is_file() || state::is_link(&meta) || meta.len() > 20 * 1024 * 1024 {
        return Err("Choose a regular file smaller than 20 MB.".into());
    }
    let name = source
        .file_name()
        .and_then(|v| v.to_str())
        .filter(|v| !v.contains(':'))
        .ok_or("Invalid attachment filename.")?
        .to_string();
    let project = state.root.join("drafts").join(id.to_string());
    state::ensure_directory(&project)?;
    let folder = project.join("attachments");
    state::ensure_directory(&folder)?;
    let filename = format!("{}-{name}", Uuid::new_v4());
    fs::copy(source, folder.join(&filename)).map_err(err)?;
    Ok(StudioAttachment {
        name,
        path: format!("attachments/{filename}"),
    })
}
