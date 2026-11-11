use crate::state::{self, Draft, InstalledMod, ModEvent, Result, State};
use bezalel_pack_sync::{self as sync, PackLock};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

// Keep file changes reversible until the matching draft update commits.
// Removed/replaced JARs stay in the pack's existing removed-mods archive.
#[derive(Default)]
struct JarChanges {
    created: Vec<PathBuf>,
    archived: Vec<(PathBuf, PathBuf)>,
    committed: bool,
}

impl JarChanges {
    fn archive(&mut self, dir: &Path, filename: &str) -> Result<()> {
        let archive = dir.join("removed-mods");
        state::ensure_directory(&archive)?;
        let source = dir.join("mods").join(filename);
        let backup = archive.join(format!("{}-{filename}", Uuid::new_v4()));
        fs::rename(&source, &backup).map_err(state::err)?;
        self.archived.push((source, backup));
        Ok(())
    }

    fn create(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
        let dir = path.parent().ok_or("Missing mod directory.")?;
        state::ensure_directory(dir)?;
        let staged = dir.join(format!(".bezalel-install-{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::File::create_new(&staged).map_err(state::err)?;
            file.write_all(bytes).map_err(state::err)?;
            file.sync_all().map_err(state::err)?;
            drop(file);
            crate::studio::platform::persist_new(&staged, path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "A mod with this filename already exists in that modpack.".to_owned()
                } else {
                    state::err(error)
                }
            })
        })();
        let _ = fs::remove_file(&staged);
        result?;
        self.created.push(path.to_owned());
        Ok(())
    }
}

impl Drop for JarChanges {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in self.created.iter().rev() {
            let _ = fs::remove_file(path);
        }
        for (source, backup) in self.archived.iter().rev() {
            let _ = fs::rename(backup, source);
        }
    }
}

fn mod_path(dir: &Path, filename: &str) -> Result<PathBuf> {
    sync::validate_artifact_path(&format!("mods/{filename}"))?;
    if filename.contains(['/', '\\', ':']) || !filename.to_ascii_lowercase().ends_with(".jar") {
        return Err("Invalid installed mod filename.".into());
    }
    state::ensure_directory(&dir.join("mods"))?;
    Ok(dir.join("mods").join(filename))
}

fn verified_bytes(dir: &Path, installed: &InstalledMod) -> Result<Vec<u8>> {
    let path = mod_path(dir, &installed.filename)?;
    let meta = fs::symlink_metadata(&path).map_err(state::err)?;
    if !meta.is_file() || state::is_link(&meta) || meta.len() > 64 * 1024 * 1024 {
        return Err("The installed mod is no longer the file Bezalel installed.".into());
    }
    let bytes = fs::read(path).map_err(state::err)?;
    if sync::sha256_hex(&bytes) != installed.sha256 {
        return Err("The installed mod has changed. Its files were left in place.".into());
    }
    Ok(bytes)
}

// The caller holds the draft/pack locks, checks game idleness, and prepares any
// loader dependency before changing the actual mod or its saved installation.
pub fn install(
    state: &State,
    draft: &Draft,
    pack_id: Uuid,
    filename: &str,
    bytes: &[u8],
    sha256: &str,
) -> Result<Draft> {
    let data = state.data()?;
    if data.drafts.iter().any(|other| {
        other.id != draft.id
            && other
                .installed_mod
                .as_ref()
                .is_some_and(|item| item.pack_id == pack_id && item.filename == filename)
    }) {
        return Err("Another Studio mod already owns this filename in that modpack.".into());
    }
    let dir = state.game_dir(&data, pack_id);
    let path = mod_path(&dir, filename)?;
    let mut changes = JarChanges::default();
    let existing = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() && !state::is_link(&meta) && meta.len() <= 64 * 1024 * 1024 => {
            Some(fs::read(&path).map_err(state::err)?)
        }
        Ok(_) => return Err("Refusing to replace a redirected mod file.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(state::err(error)),
    };
    let already_matches = existing
        .as_ref()
        .is_some_and(|old| sync::sha256_hex(old) == sha256);
    if !already_matches && existing.is_some() {
        let tracked = draft.installed_mod.as_ref().filter(|item| item.pack_id == pack_id && item.filename == filename)
            .ok_or("A different mod already uses this filename. Remove it from the modpack before installing.")?;
        verified_bytes(&dir, tracked)?;
        changes.archive(&dir, filename)?;
    }
    if let Some(old) = &draft.installed_mod {
        if old.pack_id != pack_id {
            return Err("Move the installed mod before changing its modpack.".into());
        }
        if old.filename != filename {
            verified_bytes(&dir, old)?;
            changes.archive(&dir, &old.filename)?;
        }
    }
    if !already_matches {
        changes.create(&path, bytes)?;
    }
    let updated = state.update(|data| {
        state::update_draft(data, draft.id, |draft| {
            draft.installed_mod = Some(InstalledMod {
                pack_id,
                filename: filename.into(),
                sha256: sha256.into(),
            });
            draft.clone()
        })
    })?;
    changes.committed = true;
    Ok(updated)
}

#[tauri::command]
pub async fn move_installed_mod(
    state: tauri::State<'_, State>,
    draft_id: Uuid,
    pack_id: Uuid,
) -> Result<Draft> {
    move_mod(&state, draft_id, pack_id).await
}

async fn move_mod(state: &State, draft_id: Uuid, pack_id: Uuid) -> Result<Draft> {
    let _draft_job = state.acquire(format!("draft:{draft_id}"))?;
    let data = state.data()?;
    let draft = state::draft(&data, draft_id)?;
    let installed = draft
        .installed_mod
        .ok_or("Install the mod before moving it.")?;
    if installed.pack_id == pack_id {
        return Err("This mod is already in that modpack.".into());
    }
    let source = state::pack(&data, installed.pack_id)?;
    let destination = state::pack(&data, pack_id)?;
    if source.owner_id.is_some()
        || destination.owner_id.is_some()
        || source.target_id != destination.target_id
        || destination.target_id != draft.target_id
    {
        return Err("Choose a local modpack with the same loader and Minecraft version.".into());
    }
    let _source_job = state.acquire(format!("pack:{}", installed.pack_id))?;
    let _destination_job = state.acquire(format!("pack:{pack_id}"))?;
    let data = state.data()?;
    state::pack(&data, installed.pack_id)?;
    state::pack(&data, pack_id)?;
    if data.drafts.iter().any(|other| {
        other.id != draft_id
            && other
                .installed_mod
                .as_ref()
                .is_some_and(|item| item.pack_id == pack_id && item.filename == installed.filename)
    }) {
        return Err("Another Studio mod already owns this filename in that modpack.".into());
    }
    let source_dir = state.game_dir(&data, installed.pack_id);
    let destination_dir = state.game_dir(&data, pack_id);
    let _source_lock = PackLock::acquire(&source_dir)?;
    let _destination_lock = PackLock::acquire(&destination_dir)?;
    crate::minecraft::game_process::ensure_idle(&source_dir)?;
    crate::minecraft::game_process::ensure_idle(&destination_dir)?;
    let bytes = verified_bytes(&source_dir, &installed)?;
    if state::target(&destination.target_id)?.loader == "fabric" {
        crate::packs::install_fabric_api(&destination_dir).await?;
    }
    let target = mod_path(&destination_dir, &installed.filename)?;
    let mut changes = JarChanges::default();
    changes.create(&target, &bytes)?;
    changes.archive(&source_dir, &installed.filename)?;
    let updated = state.update(|data| {
        state::update_draft(data, draft_id, |draft| {
            draft.pack_id = Some(pack_id);
            draft.installed_mod = Some(InstalledMod {
                pack_id,
                ..installed
            });
            draft.mod_events.push(ModEvent {
                after_message: draft.messages.len(),
                kind: "moved".into(),
                pack_name: destination.name,
            });
            draft.clone()
        })
    })?;
    changes.committed = true;
    Ok(updated)
}

#[tauri::command]
pub fn delete_installed_mod(state: tauri::State<'_, State>, draft_id: Uuid) -> Result<Draft> {
    delete_mod(&state, draft_id)
}

fn delete_mod(state: &State, draft_id: Uuid) -> Result<Draft> {
    let _draft_job = state.acquire(format!("draft:{draft_id}"))?;
    let data = state.data()?;
    let draft = state::draft(&data, draft_id)?;
    let installed = draft.installed_mod.ok_or("This mod is not installed.")?;
    let pack = state::pack(&data, installed.pack_id)?;
    if pack.owner_id.is_some() {
        return Err("Shared mods must be removed through a published server update.".into());
    }
    let _pack_job = state.acquire(format!("pack:{}", installed.pack_id))?;
    state::pack(&state.data()?, installed.pack_id)?;
    let dir = state.game_dir(&data, installed.pack_id);
    let _lock = PackLock::acquire(&dir)?;
    crate::minecraft::game_process::ensure_idle(&dir)?;
    verified_bytes(&dir, &installed)?;
    let mut changes = JarChanges::default();
    changes.archive(&dir, &installed.filename)?;
    let updated = state.update(|data| {
        state::update_draft(data, draft_id, |draft| {
            draft.installed_mod = None;
            draft.mod_events.push(ModEvent {
                after_message: draft.messages.len(),
                kind: "deleted".into(),
                pack_name: pack.name,
            });
            draft.clone()
        })
    })?;
    changes.committed = true;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Message, Pack};

    struct Fixture {
        state: Option<State>,
        root: PathBuf,
        source: Uuid,
        destination: Uuid,
        draft: Uuid,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("bezalel-installed-mod-{}", Uuid::new_v4()));
            let state = State::new(root.clone()).unwrap();
            let source = Uuid::new_v4();
            let destination = Uuid::new_v4();
            let draft = Uuid::new_v4();
            state
                .update(|data| {
                    for (id, name) in [(source, "Original pack"), (destination, "New pack")] {
                        data.packs.push(Pack {
                            id,
                            name: name.into(),
                            target_id: "forge-1.21.1".into(),
                            address: String::new(),
                            owner_id: None,
                            role: "owner".into(),
                            trusted_owner_id: None,
                            active_revision: None,
                            installed_revision: None,
                        });
                    }
                    data.drafts.push(Draft {
                        id: draft,
                        pack_id: Some(source),
                        title: "Fixture mod".into(),
                        target_id: "forge-1.21.1".into(),
                        mode: "direct".into(),
                        thread_id: None,
                        context_usage: None,
                        installed_mod: None,
                        mod_events: Vec::new(),
                        idea: None,
                        messages: vec![Message {
                            role: "assistant".into(),
                            text: "Mod finished.".into(),
                            choices: Vec::new(),
                        }],
                        brief: None,
                        changes: Vec::new(),
                        build: None,
                    });
                    Ok(())
                })
                .unwrap();
            Self {
                state: Some(state),
                root,
                source,
                destination,
                draft,
            }
        }
        fn state(&self) -> &State {
            self.state.as_ref().unwrap()
        }
        fn dir(&self, pack: Uuid) -> PathBuf {
            self.root.join("instances").join(pack.to_string())
        }
        fn install(&self, bytes: &[u8], filename: &str) -> Result<Draft> {
            let draft = state::draft(&self.state().data()?, self.draft)?;
            install(
                self.state(),
                &draft,
                self.source,
                filename,
                bytes,
                &sync::sha256_hex(bytes),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.state.take());
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn install_move_and_delete_persist_history_and_preserve_other_files() {
        let fixture = Fixture::new();
        let installed = fixture.install(b"verified jar", "fixture.jar").unwrap();
        assert!(installed.installed_mod.is_some());
        let reopened = State::new(fixture.root.clone()).unwrap();
        assert_eq!(
            state::draft(&reopened.data().unwrap(), fixture.draft)
                .unwrap()
                .installed_mod
                .unwrap()
                .filename,
            "fixture.jar"
        );
        drop(reopened);
        let source = fixture.dir(fixture.source);
        fs::write(source.join("mods/other.jar"), b"other mod").unwrap();
        fs::write(source.join("world.dat"), b"personal world").unwrap();
        let project = fixture.root.join("drafts").join(fixture.draft.to_string());
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Mod.java"), b"source").unwrap();

        let moved = move_mod(fixture.state(), fixture.draft, fixture.destination)
            .await
            .unwrap();
        assert_eq!(moved.pack_id, Some(fixture.destination));
        assert_eq!(moved.installed_mod.unwrap().pack_id, fixture.destination);
        assert_eq!(moved.mod_events[0].pack_name, "New pack");
        assert_eq!(moved.mod_events[0].after_message, 1);
        assert_eq!(moved.mod_events[0].kind, "moved");
        assert!(!source.join("mods/fixture.jar").exists());
        assert_eq!(
            fs::read(fixture.dir(fixture.destination).join("mods/fixture.jar")).unwrap(),
            b"verified jar"
        );
        let deleted = delete_mod(fixture.state(), fixture.draft).unwrap();
        assert!(deleted.installed_mod.is_none());
        assert_eq!(deleted.mod_events.len(), 2);
        assert_eq!(deleted.mod_events[1].kind, "deleted");
        assert_eq!(deleted.messages.len(), 1);
        assert!(!fixture
            .dir(fixture.destination)
            .join("mods/fixture.jar")
            .exists());
        assert_eq!(
            fs::read(source.join("mods/other.jar")).unwrap(),
            b"other mod"
        );
        assert_eq!(
            fs::read(source.join("world.dat")).unwrap(),
            b"personal world"
        );
        assert_eq!(fs::read(project.join("Mod.java")).unwrap(), b"source");
    }

    #[tokio::test]
    async fn failed_move_preserves_source_and_rejects_collisions_and_incompatible_packs() {
        let fixture = Fixture::new();
        fixture.install(b"verified jar", "fixture.jar").unwrap();
        let target = fixture.dir(fixture.destination).join("mods");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("fixture.jar"), b"unrelated mod").unwrap();
        assert!(
            move_mod(fixture.state(), fixture.draft, fixture.destination)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(target.join("fixture.jar")).unwrap(),
            b"unrelated mod"
        );
        assert_eq!(
            fs::read(fixture.dir(fixture.source).join("mods/fixture.jar")).unwrap(),
            b"verified jar"
        );
        let unchanged = state::draft(&fixture.state().data().unwrap(), fixture.draft).unwrap();
        assert_eq!(unchanged.pack_id, Some(fixture.source));
        assert!(unchanged.mod_events.is_empty());
        fixture
            .state()
            .update(|data| {
                data.packs
                    .iter_mut()
                    .find(|pack| pack.id == fixture.destination)
                    .unwrap()
                    .target_id = "fabric-1.21.1".into();
                Ok(())
            })
            .unwrap();
        assert!(
            move_mod(fixture.state(), fixture.draft, fixture.destination)
                .await
                .err()
                .unwrap()
                .contains("same loader")
        );
    }

    #[test]
    fn install_updates_only_its_own_unchanged_file_and_handles_renamed_builds() {
        let fixture = Fixture::new();
        let source = fixture.dir(fixture.source).join("mods");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("fixture.jar"), b"unrelated mod").unwrap();
        assert!(fixture.install(b"first build", "fixture.jar").is_err());
        assert_eq!(
            fs::read(source.join("fixture.jar")).unwrap(),
            b"unrelated mod"
        );
        fs::remove_file(source.join("fixture.jar")).unwrap();
        fixture.install(b"first build", "fixture.jar").unwrap();
        fixture.install(b"second build", "fixture.jar").unwrap();
        assert_eq!(
            fs::read(source.join("fixture.jar")).unwrap(),
            b"second build"
        );
        fixture.install(b"renamed build", "renamed.jar").unwrap();
        assert!(!source.join("fixture.jar").exists());
        assert_eq!(
            fs::read(source.join("renamed.jar")).unwrap(),
            b"renamed build"
        );
        fs::write(source.join("renamed.jar"), b"external edit").unwrap();
        assert!(delete_mod(fixture.state(), fixture.draft).is_err());
        assert!(fixture.install(b"next build", "renamed.jar").is_err());
        assert_eq!(
            fs::read(source.join("renamed.jar")).unwrap(),
            b"external edit"
        );
        assert!(fixture.install(b"bad path", "../outside.jar").is_err());
    }

    #[test]
    fn failed_state_commit_rolls_back_the_jar_changes() {
        let fixture = Fixture::new();
        let missing = Draft {
            id: Uuid::new_v4(),
            ..state::draft(&fixture.state().data().unwrap(), fixture.draft).unwrap()
        };
        assert!(install(
            fixture.state(),
            &missing,
            fixture.source,
            "fixture.jar",
            b"jar",
            &sync::sha256_hex(b"jar")
        )
        .is_err());
        assert!(!fixture
            .dir(fixture.source)
            .join("mods/fixture.jar")
            .exists());
        let dir = fixture.dir(fixture.source);
        fs::write(dir.join("mods/previous.jar"), b"previous").unwrap();
        {
            let mut changes = JarChanges::default();
            changes.archive(&dir, "previous.jar").unwrap();
            changes.create(&dir.join("mods/new.jar"), b"new").unwrap();
        }
        assert_eq!(
            fs::read(dir.join("mods/previous.jar")).unwrap(),
            b"previous"
        );
        assert!(!dir.join("mods/new.jar").exists());
    }
}
