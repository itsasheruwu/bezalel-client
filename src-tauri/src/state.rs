use crate::{builder::BuildResult, studio::StudioChange};
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub id: &'static str,
    pub loader: &'static str,
    pub minecraft_version: &'static str,
    pub loader_version: &'static str,
}

pub const TARGETS: [Target; 3] = [
    Target {
        id: "fabric-1.21.1",
        loader: "fabric",
        minecraft_version: "1.21.1",
        loader_version: "0.16.14",
    },
    Target {
        id: "forge-1.21.1",
        loader: "forge",
        minecraft_version: "1.21.1",
        loader_version: "52.1.0",
    },
    Target {
        id: "neoforge-1.21.1",
        loader: "neoforge",
        minecraft_version: "1.21.1",
        loader_version: "21.1.217",
    },
];

pub fn target(id: &str) -> Result<Target> {
    TARGETS
        .iter()
        .copied()
        .find(|target| target.id == id)
        .ok_or("This Minecraft target is unavailable.".into())
}

pub fn default_theme() -> String {
    "system".into()
}
pub fn default_model_family() -> String {
    "Sol".into()
}
pub fn default_reasoning_effort() -> String {
    "high".into()
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub onboarding_complete: bool,
    #[serde(default = "default_model_family")]
    pub model_family: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
    pub memory_mib: u32,
    pub auto_sync: bool,
    pub reduced_motion: bool,
    pub text_scale: u32,
    pub library_path: Option<PathBuf>,
    pub microsoft_client_id: String,
    pub sync_url: String,
    pub sync_key: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            onboarding_complete: false,
            model_family: default_model_family(),
            theme: default_theme(),
            reasoning_effort: default_reasoning_effort(),
            memory_mib: 4096,
            auto_sync: true,
            reduced_motion: false,
            text_scale: 100,
            library_path: None,
            microsoft_client_id: option_env!("BEZALEL_MICROSOFT_CLIENT_ID")
                .unwrap_or_default()
                .into(),
            sync_url: String::new(),
            sync_key: String::new(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: Uuid,
    pub name: String,
    pub target_id: String,
    pub address: String,
    pub owner_id: Option<Uuid>,
    pub role: String,
    pub trusted_owner_id: Option<Uuid>,
    pub active_revision: Option<u64>,
    pub installed_revision: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used_tokens: u64,
    pub window_tokens: Option<u64>,
    pub model_id: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledMod {
    pub pack_id: Uuid,
    pub filename: String,
    pub sha256: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModEvent {
    pub after_message: usize,
    pub kind: String,
    pub pack_name: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: Uuid,
    pub pack_id: Option<Uuid>,
    pub title: String,
    pub target_id: String,
    pub mode: String,
    pub thread_id: Option<String>,
    #[serde(default)]
    pub context_usage: Option<ContextUsage>,
    #[serde(default)]
    pub installed_mod: Option<InstalledMod>,
    #[serde(default)]
    pub mod_events: Vec<ModEvent>,
    #[serde(default)]
    pub idea: Option<String>,
    pub messages: Vec<Message>,
    pub brief: Option<String>,
    pub changes: Vec<StudioChange>,
    pub build: Option<BuildResult>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Data {
    pub settings: Settings,
    pub packs: Vec<Pack>,
    pub drafts: Vec<Draft>,
    pub builder_eula_accepted: bool,
    pub hidden_pack_ids: Vec<Uuid>,
}

pub struct State {
    pub root: PathBuf,
    db: Mutex<Connection>,
    active: Mutex<HashSet<String>>,
}

impl State {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).map_err(err)?;
        let db = Connection::open(root.join("bezalel.sqlite")).map_err(err)?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS app_state(id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);",
        ).map_err(err)?;
        db.execute(
            "INSERT OR IGNORE INTO app_state VALUES (1, ?1)",
            [serde_json::to_string(&Data::default()).map_err(err)?],
        )
        .map_err(err)?;
        Ok(Self {
            root,
            db: Mutex::new(db),
            active: Mutex::new(HashSet::new()),
        })
    }

    pub fn data(&self) -> Result<Data> {
        let db = self.db.lock().map_err(err)?;
        read(&db)
    }

    pub fn update<T>(&self, f: impl FnOnce(&mut Data) -> Result<T>) -> Result<T> {
        let mut db = self.db.lock().map_err(err)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let mut data = read(&tx)?;
        let value = f(&mut data)?;
        let json = serde_json::to_string(&data).map_err(err)?;
        tx.execute("UPDATE app_state SET data=?1 WHERE id=1", [json])
            .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(value)
    }

    pub fn acquire(&self, key: String) -> Result<Active<'_>> {
        if !self.active.lock().map_err(err)?.insert(key.clone()) {
            return Err("This operation is already running.".into());
        }
        Ok(Active { state: self, key })
    }

    pub fn game_dir(&self, data: &Data, id: Uuid) -> PathBuf {
        data.settings
            .library_path
            .as_ref()
            .unwrap_or(&self.root)
            .join("instances")
            .join(id.to_string())
    }
}

pub struct Active<'a> {
    state: &'a State,
    key: String,
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.state.active.lock() {
            active.remove(&self.key);
        }
    }
}

fn read(db: &Connection) -> Result<Data> {
    let json: String = db
        .query_row("SELECT data FROM app_state WHERE id=1", [], |row| {
            row.get(0)
        })
        .map_err(err)?;
    serde_json::from_str(&json).map_err(err)
}

pub fn draft(data: &Data, id: Uuid) -> Result<Draft> {
    data.drafts
        .iter()
        .find(|draft| draft.id == id)
        .cloned()
        .ok_or("Draft not found.".into())
}

pub fn update_draft<T>(data: &mut Data, id: Uuid, f: impl FnOnce(&mut Draft) -> T) -> Result<T> {
    data.drafts
        .iter_mut()
        .find(|draft| draft.id == id)
        .map(f)
        .ok_or("Draft not found.".into())
}

pub fn pack(data: &Data, id: Uuid) -> Result<Pack> {
    data.packs
        .iter()
        .find(|pack| pack.id == id)
        .cloned()
        .ok_or("Pack not found.".into())
}

pub fn err(value: impl std::fmt::Display) -> String {
    value.to_string()
}

pub fn ensure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(err)?;
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err("A managed directory must be a regular directory.".into());
    }
    Ok(())
}

pub fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}
