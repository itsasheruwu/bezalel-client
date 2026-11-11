use super::{
    allowed, archive, fetch, file_error, managed_dir, maven_path, ordinary, safe_relative,
    single_component, verified, Arguments, Download, Result, Rule, Target, MAX_ARTIFACT,
    MAX_METADATA,
};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use uuid::Uuid;

const FORGE: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge/1.21.1-52.1.0/forge-1.21.1-52.1.0-installer.jar";
const FORGE_SHA256: &str = "f1b620f2879ad6a5bbe15daba4d8f81eab9f5e08004967604960b98996bdebc4";
const NEOFORGE: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.217/neoforge-21.1.217-installer.jar";
const NEOFORGE_SHA256: &str = "2f6b728aa99646a697eb0755ce770e945f95090c4248eb58f558ac0e2e3578c6";

#[derive(Deserialize)]
struct InstallProfile {
    minecraft: String,
    version: String,
    json: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LauncherProfile {
    id: String,
    inherits_from: String,
    main_class: String,
    #[serde(default)]
    arguments: Arguments,
    libraries: Vec<LoaderLibrary>,
}

#[derive(Deserialize)]
struct LoaderLibrary {
    name: String,
    downloads: LoaderDownloads,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct LoaderDownloads {
    artifact: Option<LoaderArtifact>,
}

#[derive(Deserialize)]
struct LoaderArtifact {
    path: Option<String>,
    sha1: String,
    size: Option<u64>,
}

pub(super) struct Prepared {
    pub(super) id: String,
    pub(super) main_class: String,
    pub(super) arguments: Arguments,
    pub(super) libraries: Vec<(String, PathBuf)>,
    pub(super) directory: PathBuf,
}

fn zip_entry(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Result<Vec<u8>> {
    let entry = archive
        .by_name(name)
        .map_err(|_| "The pinned loader installer is missing its launch metadata.")?;
    if entry.size() > MAX_METADATA as u64 {
        return Err("The loader installer contains oversized launch metadata.".into());
    }
    let mut bytes = Vec::new();
    entry
        .take(MAX_METADATA as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(file_error)?;
    if bytes.len() > MAX_METADATA {
        return Err("The loader installer contains oversized launch metadata.".into());
    }
    Ok(bytes)
}

fn profiles(path: &Path, target: Target) -> Result<(LauncherProfile, Vec<u8>, PathBuf)> {
    let mut installer = archive(path)?;
    let profile: InstallProfile =
        serde_json::from_slice(&zip_entry(&mut installer, "install_profile.json")?)
            .map_err(|_| "The pinned loader installer has unsupported install metadata.")?;
    let json = profile.json.strip_prefix('/').unwrap_or(&profile.json);
    safe_relative(Path::new(json))?;
    let launch_json = zip_entry(&mut installer, json)?;
    let launcher: LauncherProfile = serde_json::from_slice(&launch_json)
        .map_err(|_| "The pinned loader installer has unsupported launch metadata.")?;
    let (id, main_class) = match target.loader {
        "forge" => (
            "1.21.1-forge-52.1.0",
            "net.minecraftforge.bootstrap.ForgeBootstrap",
        ),
        "neoforge" => (
            "neoforge-21.1.217",
            "cpw.mods.bootstraplauncher.BootstrapLauncher",
        ),
        _ => return Err("This loader does not have a pinned native profile.".into()),
    };
    if profile.minecraft != target.minecraft_version
        || launcher.inherits_from != target.minecraft_version
        || launcher.id != profile.version
        || launcher.id != id
        || launcher.main_class != main_class
    {
        return Err(
            "The installer launch metadata does not match the pinned Minecraft target.".into(),
        );
    }
    let version = single_component(&launcher.id)?;
    let json_path = Path::new("versions")
        .join(version)
        .join(format!("{}.json", launcher.id));
    Ok((launcher, launch_json, json_path))
}

async fn pinned_installer(
    client: &Client,
    cache: &Path,
    target: Target,
) -> Result<(PathBuf, String)> {
    let (url, checksum, size) = match (
        target.loader,
        target.minecraft_version,
        target.loader_version,
    ) {
        ("forge", "1.21.1", "52.1.0") => (FORGE, FORGE_SHA256, 6_377_616),
        ("neoforge", "1.21.1", "21.1.217") => (NEOFORGE, NEOFORGE_SHA256, 6_953_727),
        _ => return Err("This loader does not have a pinned native installer.".into()),
    };
    let relative = PathBuf::from(format!(
        "installers/{}-{}.jar",
        target.id, target.loader_version
    ));
    let path = fetch(
        client,
        cache,
        Download {
            url: url.into(),
            hash: checksum.into(),
            size: Some(size),
            relative,
        },
    )
    .await?;
    Ok((path, checksum.into()))
}

struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn run_installer(java: &Path, directory: &Path, installer: &Path) -> Result<()> {
    let java = java.with_file_name("java.exe");
    let home = managed_dir(directory, Path::new("home"))?;
    let temp = managed_dir(directory, Path::new("temp"))?;
    let system_root =
        std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable.")?;
    let java_property = |name: &str, path: &Path| {
        let mut value = OsString::from(name);
        value.push(path);
        value
    };
    let mut child = Command::new(java)
        .arg("-Djava.awt.headless=true")
        .arg(java_property("-Duser.home=", &home))
        .arg(java_property("-Djava.io.tmpdir=", &temp))
        .args(["-Xmx1536m", "-jar"])
        .arg(installer)
        .arg("--installClient")
        .arg(directory)
        .current_dir(directory)
        .env_clear()
        .env("SystemRoot", &system_root)
        .env("WINDIR", &system_root)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", &home)
        .env("LOCALAPPDATA", &home)
        .env("TEMP", &temp)
        .env("TMP", &temp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Could not start the verified loader installer.")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("The loader installer did not expose its output.")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("The loader installer did not expose its errors.")?;
    let result = timeout(Duration::from_secs(300), async {
        tokio::try_join!(read_output(stdout), read_output(stderr), async {
            child.wait().await.map_err(file_error)
        })
    })
    .await;
    match result {
        Ok(Ok(((), (), status))) if status.success() => Ok(()),
        Ok(Ok(_)) => Err("The verified loader installer failed. You can still use the official launcher handoff.".into()),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err("The loader installer timed out. You can retry or use the official launcher handoff.".into())
        }
    }
}

async fn read_output(stream: impl tokio::io::AsyncRead + Unpin) -> Result<()> {
    const LIMIT: u64 = 1024 * 1024;
    let size = tokio::io::copy(&mut stream.take(LIMIT + 1), &mut tokio::io::sink())
        .await
        .map_err(file_error)?;
    if size > LIMIT {
        return Err("The loader installer exceeded its output limit.".into());
    }
    Ok(())
}

fn regular(path: &Path) -> Result<fs::Metadata> {
    let info = fs::symlink_metadata(path).map_err(file_error)?;
    if !info.is_file() || !ordinary(&info) || info.len() == 0 || info.len() > MAX_ARTIFACT {
        return Err("An installed loader file is redirected, empty, or oversized.".into());
    }
    Ok(info)
}

fn file_tree(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let info = fs::symlink_metadata(directory).map_err(file_error)?;
    if !info.is_dir() || !ordinary(&info) {
        return Err("An installed loader directory is redirected.".into());
    }
    for entry in fs::read_dir(directory).map_err(file_error)? {
        let path = entry.map_err(file_error)?.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "Invalid loader cache path.")?;
        safe_relative(relative)?;
        let info = fs::symlink_metadata(&path).map_err(file_error)?;
        if !ordinary(&info) || relative.components().count() > 24 || files.len() >= 4096 {
            return Err(
                "The installed loader cache exceeds its path limits or contains a link.".into(),
            );
        }
        if info.is_dir() {
            file_tree(root, &path, files)?;
        } else {
            regular(&path)?;
            files.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn receipt(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    file_tree(root, &root.join("libraries"), &mut files)?;
    file_tree(root, &root.join("versions"), &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    let mut total: u64 = 0;
    let mut buffer = [0; 64 * 1024];
    for relative in files {
        let path = root.join(&relative);
        let size = regular(&path)?.len();
        let relative = relative
            .to_str()
            .ok_or("A loader cache path is invalid Unicode.")?;
        total += size;
        if total > 2 * 1024 * 1024 * 1024 {
            return Err("The installed loader files exceed the cache size limit.".into());
        }
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(relative.as_bytes());
        digest.update(size.to_le_bytes());
        let mut file = fs::File::open(path).map_err(file_error)?.take(size + 1);
        let mut remaining = size;
        loop {
            let count = file.read(&mut buffer).map_err(file_error)?;
            if count == 0 {
                break;
            }
            remaining = remaining
                .checked_sub(count as u64)
                .ok_or("An installed loader file changed.")?;
            digest.update(&buffer[..count]);
        }
        if remaining != 0 {
            return Err("An installed loader file changed during verification.".into());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

async fn cache_receipt(root: &Path) -> Result<String> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || receipt(&root))
        .await
        .map_err(|_| "Loader cache verification stopped.".to_string())?
}

fn exact_profile(root: &Path, relative: &Path, expected: &[u8]) -> Result<()> {
    let path = root.join(relative);
    let info = regular(&path)?;
    if info.len() != expected.len() as u64 || fs::read(path).map_err(file_error)? != expected {
        return Err("The installed loader profile differs from its publisher's installer.".into());
    }
    Ok(())
}

async fn checked_libraries(
    root: &Path,
    profile: &LauncherProfile,
) -> Result<Vec<(String, PathBuf)>> {
    let mut classpath = Vec::new();
    for library in &profile.libraries {
        if !allowed(&library.rules, false)? {
            continue;
        }
        let Some(artifact) = &library.downloads.artifact else {
            return Err("The pinned loader profile uses an unsupported library artifact.".into());
        };
        let relative = match &artifact.path {
            Some(path) => PathBuf::from(path),
            None => maven_path(&library.name)?,
        };
        safe_relative(&relative)?;
        let relative = Path::new("libraries").join(relative);
        let path = root.join(&relative);
        let job = Download {
            url: String::new(),
            hash: artifact.sha1.clone(),
            size: artifact.size.filter(|size| *size > 0),
            relative,
        };
        if !verified(&path, &job).await? {
            return Err("An installed loader library failed its publisher checksum.".into());
        }
        classpath.push((library.name.clone(), path));
    }
    if classpath.is_empty() {
        return Err("The pinned loader installer returned an empty classpath.".into());
    }
    Ok(classpath)
}

pub(super) async fn prepare(
    client: &Client,
    cache: &Path,
    java: &Path,
    target: Target,
    vanilla_version: &Path,
    vanilla_client: &Path,
    emit: &impl Fn(String),
) -> Result<Prepared> {
    let (installer, checksum) = pinned_installer(client, cache, target).await?;
    let (profile, expected_profile, profile_path) = profiles(&installer, target)?;
    let directory = managed_dir(cache, Path::new("installed-loaders"))?;
    let destination = directory.join(format!(
        "{}-{}-{checksum}",
        target.id, target.loader_version
    ));
    if fs::symlink_metadata(&destination).is_ok() {
        let name = destination
            .file_name()
            .ok_or("The installed loader has an invalid cache name.")?;
        managed_dir(&directory, Path::new(name))?;
        let path = destination.join("bezalel-receipt.sha256");
        if regular(&path)?.len() != 64 {
            return Err(
                "The cached loader receipt is invalid. Remove its managed folder and retry.".into(),
            );
        }
        let expected = fs::read_to_string(path).map_err(file_error)?;
        if cache_receipt(&destination).await? != expected {
            return Err(
                "The cached loader failed verification. Remove its managed folder and retry."
                    .into(),
            );
        }
    } else {
        let stage = Staging(directory.join(format!("staging-{}", Uuid::new_v4())));
        fs::create_dir(&stage.0).map_err(file_error)?;
        let installer_in_stage = stage.0.join("loader-installer.jar");
        fs::copy(installer, &installer_in_stage).map_err(file_error)?;
        fs::write(
            stage.0.join("launcher_profiles.json"),
            br#"{"profiles":{}}"#,
        )
        .map_err(file_error)?;
        let game = managed_dir(
            &stage.0,
            &Path::new("versions").join(target.minecraft_version),
        )?;
        fs::copy(
            vanilla_version,
            game.join(format!("{}.json", target.minecraft_version)),
        )
        .map_err(file_error)?;
        fs::copy(
            vanilla_client,
            game.join(format!("{}.jar", target.minecraft_version)),
        )
        .map_err(file_error)?;
        emit(format!("Installing {} in its own cache", target.loader));
        run_installer(java, &stage.0, &installer_in_stage).await?;
        exact_profile(&stage.0, &profile_path, &expected_profile)?;
        checked_libraries(&stage.0, &profile).await?;
        let digest = cache_receipt(&stage.0).await?;
        fs::write(stage.0.join("bezalel-receipt.sha256"), digest).map_err(file_error)?;
        fs::remove_file(installer_in_stage).map_err(file_error)?;
        fs::rename(&stage.0, &destination).map_err(file_error)?;
    }
    exact_profile(&destination, &profile_path, &expected_profile)?;
    let libraries = checked_libraries(&destination, &profile).await?;
    Ok(Prepared {
        id: profile.id,
        main_class: profile.main_class,
        arguments: profile.arguments,
        libraries,
        directory: destination.join("libraries"),
    })
}
