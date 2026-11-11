use crate::{
    state::{ensure_directory, err, is_link, Result},
    studio::StudioChange,
};
use std::{collections::HashSet, fs, path::Path};
use uuid::Uuid;

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;

pub fn apply(root: &Path, id: Uuid, changes: &[StudioChange]) -> Result<()> {
    if changes.is_empty() || changes.len() > 128 {
        return Err("The agent must propose between 1 and 128 source changes.".into());
    }
    let mut seen = HashSet::new();
    let mut size = 0;
    for change in changes {
        if !valid_path(&change.path) || !seen.insert(change.path.to_ascii_lowercase()) {
            return Err(format!(
                "The agent proposed an invalid or duplicate source path: {}",
                change.path
            ));
        }
        size += change.content.as_ref().map_or(0, String::len);
        if size > MAX_SOURCE_BYTES {
            return Err("The proposed source changes exceed the draft size limit.".into());
        }
    }
    let drafts = root.join("drafts");
    ensure_directory(&drafts)?;
    let draft = drafts.join(id.to_string());
    ensure_directory(&draft)?;
    let src = draft.join("src");
    let backup = draft.join("src.previous");
    if backup.exists() && !src.exists() {
        fs::rename(&backup, &src).map_err(err)?;
    }
    if backup.exists() {
        remove_tree(&backup)?;
    }
    let stage = draft.join(format!("src-stage-{}", Uuid::new_v4()));
    fs::create_dir(&stage).map_err(err)?;
    let result = (|| {
        if src.exists() {
            copy_tree(&src, &stage)?;
        }
        for change in changes {
            let file = stage.join(
                change
                    .path
                    .strip_prefix("src/")
                    .ok_or("Invalid source path.")?,
            );
            if let Some(content) = &change.content {
                fs::create_dir_all(file.parent().ok_or("Invalid source path.")?).map_err(err)?;
                fs::write(file, content).map_err(err)?;
            } else if file.exists() {
                fs::remove_file(file).map_err(err)?;
            }
        }
        if src.exists() {
            fs::rename(&src, &backup).map_err(err)?;
        }
        if let Err(error) = fs::rename(&stage, &src) {
            if backup.exists() {
                fs::rename(&backup, &src).map_err(err)?;
            }
            return Err(err(error));
        }
        Ok(())
    })();
    if stage.exists() {
        let _ = remove_tree(&stage);
    }
    if src.exists() && backup.exists() {
        let _ = remove_tree(&backup);
    }
    result
}

fn valid_path(path: &str) -> bool {
    let allowed_root =
        path.starts_with("src/main/java/") || path.starts_with("src/main/resources/");
    allowed_root
        && path.len() <= 220
        && path.split('/').all(|part| {
            let stem = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let reserved = matches!(stem.as_str(), "con" | "aux" | "nul" | "prn")
                || (stem.len() == 4
                    && (stem.starts_with("com") || stem.starts_with("lpt"))
                    && stem.as_bytes()[3].is_ascii_digit());
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !reserved
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        })
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(from).map_err(err)?;
    if is_link(&metadata) {
        return Err("Linked files are not allowed in a mod draft.".into());
    }
    if metadata.is_file() {
        fs::copy(from, to).map_err(err)?;
    } else if metadata.is_dir() {
        ensure_directory(to)?;
        for entry in fs::read_dir(from).map_err(err)? {
            let entry = entry.map_err(err)?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        return Err("Unexpected file type in mod draft.".into());
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if is_link(&metadata) || !metadata.is_dir() {
        return Err("Refusing to remove a linked draft directory.".into());
    }
    fs::remove_dir_all(path).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::valid_path;

    #[test]
    fn limits_edits_to_safe_source_paths() {
        assert!(valid_path("src/main/java/com/bezalel/Glowcap.java"));
        assert!(valid_path("src/main/resources/META-INF/neoforge.mods.toml"));
        for path in [
            "build.gradle",
            "src/main/java/../../outside",
            "src/main/java/NUL.txt",
            "src/main/java/A.java:stream",
            "src/main/java/COM1/Thing.java",
        ] {
            assert!(!valid_path(path), "{path}");
        }
    }
}
