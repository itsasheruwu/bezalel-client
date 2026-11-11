use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{Artifact, SignedRelease, SyncError, sha256_hex};

pub struct PackLock {
    root: PathBuf,
    work: PathBuf,
    _file: File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRelease {
    pub pack_id: Uuid,
    pub owner_id: Uuid,
    pub revision: u64,
    pub signature: String,
    installation_id: Uuid,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    target: InstalledRelease,
    mods_existed: bool,
    config_existed: bool,
}

impl PackLock {
    pub fn acquire(root: &Path) -> Result<Self, SyncError> {
        fs::create_dir_all(root).map_err(fs_error)?;
        require_directory(root)?;
        let lock_path = root.join(".bezalel-pack.lock");
        optional_file(&lock_path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(fs_error)?;
        file.try_lock()
            .map_err(|_| "This pack is in use. Close Minecraft or wait for sync.")?;
        let work = root.join(".bezalel-sync");
        create_directory(&work)?;
        let lock = Self {
            root: root.to_owned(),
            work,
            _file: file,
        };
        lock.recover()?;
        lock.discard_stage()?;
        Ok(lock)
    }

    pub fn installed(&self) -> Result<Option<InstalledRelease>, SyncError> {
        read_json(&self.work.join("installed.json"))
    }

    pub(crate) fn prepare(&self) -> Result<PathBuf, SyncError> {
        self.recover()?;
        let staged = self.work.join("stage");
        self.discard_stage()?;
        create_directory(&staged)?;
        create_directory(&staged.join("mods"))?;
        create_directory(&staged.join("config"))?;
        create_directory(&staged.join("config").join("bezalel"))?;
        Ok(staged)
    }

    pub(crate) fn discard_stage(&self) -> Result<(), SyncError> {
        remove_directory(&self.work.join("stage"))
    }

    pub(crate) fn matches(&self, release: &SignedRelease) -> Result<bool, SyncError> {
        let mut actual_paths = Vec::new();
        let config_root = self.root.join("config");
        optional_directory(&config_root)?;
        for folder in ["mods", "config/bezalel"] {
            let directory = self.root.join(folder);
            if !optional_directory(&directory)? {
                return Ok(false);
            }
            for item in fs::read_dir(&directory).map_err(fs_error)? {
                let item = item.map_err(fs_error)?;
                if !optional_file(&item.path()).is_ok_and(|file| file.is_some()) {
                    return Ok(false);
                }
                let Some(name) = item.file_name().into_string().ok() else {
                    return Ok(false);
                };
                actual_paths.push(format!("{folder}/{name}"));
            }
        }
        let files = release
            .manifest
            .files
            .iter()
            .filter(|entry| entry.side.client());
        let mut expected_paths: Vec<_> = files.clone().map(|entry| entry.path.as_str()).collect();
        expected_paths.sort_unstable();
        actual_paths.sort_unstable();
        if !actual_paths.iter().map(String::as_str).eq(expected_paths) {
            return Ok(false);
        }
        for entry in files {
            if self.cached(entry)?.is_none() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn cached(&self, artifact: &Artifact) -> Result<Option<Vec<u8>>, SyncError> {
        let path = self.root.join(&artifact.path);
        if artifact.path.starts_with("config/") && !optional_directory(&self.root.join("config"))? {
            return Ok(None);
        }
        if !optional_directory(path.parent().ok_or("Invalid artifact parent")?)? {
            return Ok(None);
        }
        let Some(metadata) = optional_file(&path)? else {
            return Ok(None);
        };
        if metadata.len() != artifact.bytes {
            return Ok(None);
        }
        let bytes = fs::read(path).map_err(fs_error)?;
        if bytes.len() as u64 == artifact.bytes && sha256_hex(&bytes) == artifact.sha256 {
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn activate(&self, release: &SignedRelease) -> Result<InstalledRelease, SyncError> {
        let target = InstalledRelease {
            pack_id: release.manifest.pack_id,
            owner_id: release.manifest.owner_id,
            revision: release.manifest.revision,
            signature: release.signature.clone(),
            installation_id: Uuid::new_v4(),
        };
        let config_root = self.root.join("config");
        create_directory(&config_root)?;
        let mods = self.root.join("mods");
        let config = config_root.join("bezalel");
        let pending = Pending {
            target: target.clone(),
            mods_existed: optional_directory(&mods)?,
            config_existed: optional_directory(&config)?,
        };
        let staged = self.work.join("stage");
        let outcome = (|| {
            write_json(&self.work.join("pending.json"), &pending)?;
            swap(
                &mods,
                &staged.join("mods"),
                &self.work.join("previous-mods"),
                pending.mods_existed,
            )?;
            swap(
                &config,
                &staged.join("config").join("bezalel"),
                &self.work.join("previous-config"),
                pending.config_existed,
            )?;
            write_json(&self.work.join("installed.json"), &target)?;
            Ok(target)
        })();
        match (outcome, self.recover()) {
            (Ok(target), Ok(())) => Ok(target),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(recovery)) => {
                Err(format!("{error}. Recovery needs attention: {recovery}"))
            }
        }
    }

    fn recover(&self) -> Result<(), SyncError> {
        let journal = self.work.join("pending.json");
        let Some(pending) = read_json::<Pending>(&journal)? else {
            return Ok(());
        };
        let backup_mods = self.work.join("previous-mods");
        let backup_config = self.work.join("previous-config");
        let committed = self
            .installed()?
            .is_some_and(|installed| installed.installation_id == pending.target.installation_id);
        if !committed {
            let config_root = self.root.join("config");
            require_directory(&config_root)?;
            restore(&self.root.join("mods"), &backup_mods, pending.mods_existed)?;
            restore(
                &config_root.join("bezalel"),
                &backup_config,
                pending.config_existed,
            )?;
        }
        remove_directory(&backup_mods)?;
        remove_directory(&backup_config)?;
        remove_directory(&self.work.join("stage"))?;
        fs::remove_file(journal).map_err(fs_error)
    }
}

fn swap(live: &Path, staged: &Path, previous: &Path, existed: bool) -> Result<(), SyncError> {
    require_directory(staged)?;
    if existed {
        fs::rename(live, previous).map_err(fs_error)?;
    }
    fs::rename(staged, live).map_err(fs_error)
}

fn restore(live: &Path, previous: &Path, existed: bool) -> Result<(), SyncError> {
    if optional_directory(previous)? {
        remove_directory(live)?;
        fs::rename(previous, live).map_err(fs_error)?;
    } else if !existed {
        remove_directory(live)?;
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), SyncError> {
    let next = path.with_extension("next");
    if optional_file(&next)?.is_some() {
        fs::remove_file(&next).map_err(fs_error)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&next)
        .map_err(fs_error)?;
    serde_json::to_writer(&file, value).map_err(|error| error.to_string())?;
    file.sync_all().map_err(fs_error)?;
    drop(file);
    fs::rename(next, path).map_err(fs_error)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, SyncError> {
    let Some(metadata) = optional_file(path)? else {
        return Ok(None);
    };
    if metadata.len() > 8192 {
        return Err("Invalid pack sync state".into());
    }
    let bytes = fs::read(path).map_err(fs_error)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Invalid pack sync state".into())
}

fn create_directory(path: &Path) -> Result<(), SyncError> {
    if !optional_directory(path)? {
        fs::create_dir(path).map_err(fs_error)?;
    }
    Ok(())
}

fn remove_directory(path: &Path) -> Result<(), SyncError> {
    if optional_directory(path)? {
        fs::remove_dir_all(path).map_err(fs_error)?;
    }
    Ok(())
}

fn require_directory(path: &Path) -> Result<(), SyncError> {
    if !optional_directory(path)? {
        return Err(format!("Expected pack directory {}", path.display()));
    }
    Ok(())
}

fn optional_directory(path: &Path) -> Result<bool, SyncError> {
    match metadata(path)? {
        Some(entry) if entry.is_dir() => Ok(true),
        Some(_) => Err(format!("Expected pack directory {}", path.display())),
        None => Ok(false),
    }
}

fn optional_file(path: &Path) -> Result<Option<fs::Metadata>, SyncError> {
    match metadata(path)? {
        Some(entry) if entry.is_file() => Ok(Some(entry)),
        Some(_) => Err(format!("Expected pack file {}", path.display())),
        None => Ok(None),
    }
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>, SyncError> {
    match fs::symlink_metadata(path) {
        Ok(entry) if is_link(&entry) => {
            Err(format!("Pack path cannot be a link: {}", path.display()))
        }
        Ok(entry) => Ok(Some(entry)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fs_error(error)),
    }
}

fn is_link(entry: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        return entry.file_attributes() & REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    entry.file_type().is_symlink()
}

fn fs_error(error: io::Error) -> SyncError {
    format!("Pack storage error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("bezalel-sync-test-{}", Uuid::new_v4())))
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn interrupted_swap_recovers_old_mods_without_touching_saves() {
        let directory = TestDirectory::new();
        let lock = PackLock::acquire(&directory.0).unwrap();
        assert!(PackLock::acquire(&directory.0).is_err());
        let mods = directory.0.join("mods");
        let config = directory.0.join("config");
        let saves = directory.0.join("saves");
        create_directory(&mods).unwrap();
        create_directory(&config).unwrap();
        create_directory(&saves).unwrap();
        fs::write(mods.join("old.jar"), b"old").unwrap();
        fs::write(saves.join("world"), b"world").unwrap();
        let staged = lock.prepare().unwrap();
        fs::write(staged.join("mods/new.jar"), b"new").unwrap();
        let target = InstalledRelease {
            pack_id: Uuid::new_v4(),
            owner_id: Uuid::new_v4(),
            revision: 1,
            signature: "target".into(),
            installation_id: Uuid::new_v4(),
        };
        write_json(
            &lock.work.join("pending.json"),
            &Pending {
                target,
                mods_existed: true,
                config_existed: false,
            },
        )
        .unwrap();
        swap(
            &mods,
            &staged.join("mods"),
            &lock.work.join("previous-mods"),
            true,
        )
        .unwrap();
        drop(lock);
        let lock = PackLock::acquire(&directory.0).unwrap();
        assert_eq!(fs::read(mods.join("old.jar")).unwrap(), b"old");
        assert!(!mods.join("new.jar").exists());
        assert_eq!(fs::read(saves.join("world")).unwrap(), b"world");
        assert!(lock.installed().unwrap().is_none());
    }
}
