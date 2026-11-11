use super::Profile;
use crate::state::{Result, Target};
use reqwest::{Client, Url};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::Mutex,
    task::JoinSet,
};
use uuid::Uuid;

#[path = "installer.rs"]
mod installer;

const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const JAVA_URL: &str = "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jre_x64_windows_hotspot_21.0.12.1_1.zip";
const JAVA_DIGEST: &str = "d35f31e712f0fcf6ac5a093edc90204fbff22f720ba3950bd09d331d5e621636";
const JAVA_DIR: &str = "temurin-21.0.12.1-1";
const JAVA_ARCHIVE_DIR: &str = "jdk-21.0.12.1+1-jre";
const MAX_METADATA: usize = 16 * 1024 * 1024;
const MAX_ARTIFACT: u64 = 256 * 1024 * 1024;
static INSTALL: Mutex<()> = Mutex::const_new(());

#[derive(Deserialize)]
struct Catalog {
    versions: Vec<CatalogVersion>,
}

#[derive(Deserialize)]
struct CatalogVersion {
    id: String,
    url: String,
    sha1: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Game {
    downloads: GameDownloads,
    asset_index: IndexedArtifact,
    libraries: Vec<Library>,
    arguments: Arguments,
    #[serde(default)]
    logging: Option<Logging>,
    #[serde(rename = "type")]
    version_type: String,
}

#[derive(Deserialize)]
struct GameDownloads {
    client: Artifact,
}

#[derive(Deserialize)]
struct IndexedArtifact {
    id: String,
    #[serde(flatten)]
    file: Artifact,
}

#[derive(Deserialize)]
struct Logging {
    client: ClientLogging,
}

#[derive(Deserialize)]
struct ClientLogging {
    argument: String,
    file: IndexedArtifact,
}

#[derive(Clone, Deserialize)]
struct Artifact {
    url: String,
    sha1: String,
    size: u64,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct Assets {
    objects: BTreeMap<String, Asset>,
}

#[derive(Deserialize)]
struct Asset {
    hash: String,
    size: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FabricProfile {
    id: String,
    inherits_from: String,
    main_class: String,
    libraries: Vec<FabricLibrary>,
    arguments: Arguments,
}

#[derive(Deserialize)]
struct FabricLibrary {
    name: String,
    url: String,
    sha1: Option<String>,
    sha256: Option<String>,
    size: Option<u64>,
}

#[derive(Deserialize)]
struct Library {
    name: String,
    downloads: LibraryDownloads,
    #[serde(default)]
    rules: Vec<Rule>,
    #[serde(default)]
    natives: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct LibraryDownloads {
    artifact: Option<Artifact>,
    #[serde(default)]
    classifiers: BTreeMap<String, Artifact>,
}

#[derive(Default, Deserialize)]
struct Arguments {
    #[serde(default)]
    game: Vec<Argument>,
    #[serde(default)]
    jvm: Vec<Argument>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Argument {
    Literal(String),
    Conditional { rules: Vec<Rule>, value: Values },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Values {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Rule {
    action: String,
    os: Option<Platform>,
    #[serde(default)]
    features: BTreeMap<String, bool>,
}

#[derive(Deserialize)]
struct Platform {
    name: Option<String>,
    arch: Option<String>,
    version: Option<String>,
}

fn allowed(rules: &[Rule], quick_play: bool) -> Result<bool> {
    let mut allowed = rules.is_empty();
    for rule in rules {
        if let Some(os) = &rule.os {
            if os.name.as_deref().is_some_and(|name| name != "windows")
                || os
                    .arch
                    .as_deref()
                    .is_some_and(|arch| !matches!(arch, "amd64" | "x86_64"))
            {
                continue;
            }
            if let Some(version) = os.version.as_deref() {
                // The alpha targets Windows 11, whose NT version is 10.0.
                match version {
                    r"^10\." | r"^10\.0" | r"^10\.0\." => {}
                    r"^6\." | r"^6\.1" | r"^6\.2" | r"^6\.3" | r"^6\.1\." | r"^6\.2\."
                    | r"^6\.3\." => continue,
                    _ => {
                        return Err(
                            "Minecraft requires a Windows version rule Bezalel does not recognize."
                                .into(),
                        )
                    }
                }
            }
        }
        if !rule.features.iter().all(|(name, expected)| {
            let active = match name.as_str() {
                "has_quick_plays_support" | "is_quick_play_multiplayer" => quick_play,
                _ => false,
            };
            active == *expected
        }) {
            continue;
        }
        allowed = match rule.action.as_str() {
            "allow" => true,
            "disallow" => false,
            _ => return Err("Minecraft supplied an invalid platform rule.".into()),
        };
    }
    Ok(allowed)
}

fn launch_arguments(arguments: &[Argument], quick_play: bool) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for arg in arguments {
        match arg {
            Argument::Literal(value) => result.push(value.clone()),
            Argument::Conditional { rules, value } if allowed(rules, quick_play)? => match value {
                Values::One(value) => result.push(value.clone()),
                Values::Many(values) => result.extend(values.iter().cloned()),
            },
            Argument::Conditional { .. } => {}
        }
    }
    Ok(result)
}

fn expand(arg: &str, values: &BTreeMap<&str, String>) -> Result<String> {
    let mut expanded = String::new();
    let mut rest = arg;
    while let Some((head, tail)) = rest.split_once("${") {
        expanded.push_str(head);
        let (key, remaining) = tail
            .split_once('}')
            .ok_or("Invalid Minecraft argument template.")?;
        expanded.push_str(
            values
                .get(key)
                .ok_or("Minecraft requested an unsupported launch argument.")?,
        );
        rest = remaining;
    }
    expanded.push_str(rest);
    Ok(expanded)
}

fn java_argfile(arguments: &[String]) -> Result<String> {
    let mut output = String::new();
    for argument in arguments {
        if argument.contains('\0') {
            return Err("A Java argument contains an invalid character.".into());
        }
        output.push('"');
        for character in argument.chars() {
            match character {
                '\\' => output.push_str("\\\\"),
                '"' => output.push_str("\\\""),
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                '\u{000c}' => output.push_str("\\f"),
                value => output.push(value),
            }
        }
        output.push_str("\"\n");
    }
    Ok(output)
}

#[derive(Clone)]
struct Download {
    url: String,
    hash: String,
    size: Option<u64>,
    relative: PathBuf,
}

impl Download {
    fn official(file: Artifact, relative: PathBuf) -> Self {
        Self {
            url: file.url,
            hash: file.sha1,
            size: Some(file.size),
            relative,
        }
    }
}

enum Checksum {
    Sha1(Sha1),
    Sha256(Sha256),
}

impl Checksum {
    fn for_hash(hash: &str) -> Result<Self> {
        if !hash.bytes().all(|value| value.is_ascii_hexdigit()) {
            return Err("The publisher supplied an invalid checksum.".into());
        }
        match hash.len() {
            40 => Ok(Self::Sha1(Sha1::new())),
            64 => Ok(Self::Sha256(Sha256::new())),
            _ => Err("The publisher supplied an unsupported checksum.".into()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Sha1(hash) => hash.update(bytes),
            Self::Sha256(hash) => hash.update(bytes),
        }
    }

    fn matches(self, expected: &str) -> bool {
        let digest = match self {
            Self::Sha1(hash) => format!("{:x}", hash.finalize()),
            Self::Sha256(hash) => format!("{:x}", hash.finalize()),
        };
        digest.eq_ignore_ascii_case(expected)
    }
}

fn official_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && matches!(
            url.host_str(),
            Some(
                "piston-meta.mojang.com"
                    | "piston-data.mojang.com"
                    | "launchermeta.mojang.com"
                    | "launcher.mojang.com"
                    | "libraries.minecraft.net"
                    | "resources.download.minecraft.net"
                    | "meta.fabricmc.net"
                    | "maven.fabricmc.net"
                    | "maven.minecraftforge.net"
                    | "maven.neoforged.net"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

fn download_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(240))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 5 && official_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("Unapproved download redirect")
            }
        }))
        .build()
        .map_err(|_| "Could not initialize the Minecraft download client.".into())
}

async fn response(client: &Client, url: &str) -> Result<reqwest::Response> {
    let url = Url::parse(url).map_err(|_| "Invalid publisher download URL.")?;
    if !official_url(&url) {
        return Err("Minecraft metadata supplied an unapproved download host.".into());
    }
    let host = url.host_str().unwrap_or_default().to_string();
    client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| match error.status() {
            Some(status) => format!("{host} returned HTTP {status} for a required download."),
            None => format!("Could not download a required file from {host}."),
        })
}

async fn metadata(client: &Client, url: &str, max: usize) -> Result<Vec<u8>> {
    let mut response = response(client, url).await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Metadata download was interrupted.")?
    {
        if chunk.len() > max.saturating_sub(bytes.len()) {
            return Err("Publisher metadata is larger than expected.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn file_error(error: io::Error) -> String {
    format!("Minecraft file operation failed ({:?}).", error.kind())
}

fn ordinary(info: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if info.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !info.file_type().is_symlink()
}

fn safe_relative(path: &Path) -> Result<()> {
    let mut components = path.components().peekable();
    if components.peek().is_none() {
        return Err("An empty archive or download path is invalid.".into());
    }
    for component in components {
        let Component::Normal(component) = component else {
            return Err("An archive or download path escaped its destination.".into());
        };
        let name = component
            .to_str()
            .ok_or("A download path contains invalid Unicode.")?;
        let reserved = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if name.ends_with([' ', '.'])
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b" ._+-@".contains(&c))
            || matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((reserved.starts_with("COM") || reserved.starts_with("LPT"))
                && reserved.len() == 4
                && reserved.as_bytes()[3].is_ascii_digit())
        {
            return Err("An archive or download path contains an unsafe filename.".into());
        }
    }
    Ok(())
}

fn managed_dir(base: &Path, relative: &Path) -> Result<PathBuf> {
    let mut path = base.to_path_buf();
    if relative.as_os_str().is_empty() {
        return Ok(path);
    }
    safe_relative(relative)?;
    for part in relative.components() {
        path.push(part.as_os_str());
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let info = fs::symlink_metadata(&path).map_err(file_error)?;
                if !info.is_dir() || !ordinary(&info) {
                    return Err(
                        "A Minecraft cache directory is redirected or is not a directory.".into(),
                    );
                }
            }
            Err(error) => return Err(file_error(error)),
        }
    }
    Ok(path)
}

async fn verified(path: &Path, job: &Download) -> Result<bool> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) if info.is_file() && ordinary(&info) => info,
        Ok(_) => {
            return Err("A Minecraft cache file is redirected or is not a regular file.".into())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(file_error(error)),
    };
    if info.len() > MAX_ARTIFACT || job.size.is_some_and(|size| info.len() != size) {
        return Ok(false);
    }
    let mut file = tokio::fs::File::open(path).await.map_err(file_error)?;
    let mut hash = Checksum::for_hash(&job.hash)?;
    let mut buf = vec![0; 64 * 1024];
    let mut total = 0;
    loop {
        let size = file.read(&mut buf).await.map_err(file_error)?;
        if size == 0 {
            break;
        }
        total += size as u64;
        if total > MAX_ARTIFACT {
            return Ok(false);
        }
        hash.update(&buf[..size]);
    }
    Ok(job.size.is_none_or(|size| total == size) && hash.matches(&job.hash))
}

async fn fetch(client: &Client, base: &Path, job: Download) -> Result<PathBuf> {
    safe_relative(&job.relative)?;
    let mut hash = Checksum::for_hash(&job.hash)?;
    let parent = managed_dir(
        base,
        job.relative.parent().ok_or("Missing download parent.")?,
    )?;
    let path = base.join(&job.relative);
    if verified(&path, &job).await? {
        return Ok(path);
    }
    let max = job.size.unwrap_or(MAX_ARTIFACT).min(MAX_ARTIFACT);
    let mut response = response(client, &job.url).await?;
    let part = PartialDownload(parent.join(format!("{}.part", Uuid::new_v4())));
    let result = async {
        let mut file = tokio::fs::File::create_new(&part.0)
            .await
            .map_err(file_error)?;
        let mut total = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "A Minecraft download was interrupted.")?
        {
            total += chunk.len() as u64;
            if total > max {
                return Err("A Minecraft download exceeded its expected size.".into());
            }
            hash.update(&chunk);
            file.write_all(&chunk).await.map_err(file_error)?;
        }
        if !hash.matches(&job.hash) || job.size.is_some_and(|size| total != size) {
            return Err("A Minecraft download failed its publisher checksum or size check.".into());
        }
        file.flush().await.map_err(file_error)?;
        drop(file);
        if path.exists() {
            if verified(&path, &job).await? {
                return Ok(());
            }
            fs::remove_file(&path).map_err(file_error)?;
        }
        fs::rename(&part.0, &path).map_err(file_error)
    }
    .await;
    result?;
    Ok(path)
}

struct PartialDownload(PathBuf);

impl Drop for PartialDownload {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

async fn fetch_all(
    client: &Client,
    base: &Path,
    jobs: &[Download],
    emit: &impl Fn(String),
    label: &str,
) -> Result<()> {
    let total = jobs.len();
    for (index, group) in jobs.chunks(12).enumerate() {
        if index == 0 || index % 12 == 0 {
            emit(format!("{label} · {} / {total}", index * 12));
        }
        let mut downloads = JoinSet::new();
        for job in group {
            let (client, base, job) = (client.clone(), base.to_path_buf(), job.clone());
            downloads.spawn(async move { fetch(&client, &base, job).await });
        }
        let mut first_error = None;
        while let Some(done) = downloads.join_next().await {
            if let Err(error) = done
                .map_err(|_| "A Minecraft download task stopped.".to_string())
                .and_then(|result| result)
            {
                first_error.get_or_insert(error);
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    Ok(())
}

fn file_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = fs::File::open(path).map_err(file_error)?;
    serde_json::from_reader(file.take(MAX_METADATA as u64))
        .map_err(|_| "A verified Minecraft metadata file uses an unsupported format.".into())
}

fn single_component(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    safe_relative(&path)?;
    if path.components().count() != 1 {
        return Err("A Minecraft metadata name is not a filename.".into());
    }
    Ok(path)
}

fn archive(path: &Path) -> Result<zip::ZipArchive<fs::File>> {
    zip::ZipArchive::new(fs::File::open(path).map_err(file_error)?)
        .map_err(|_| "A verified archive is not a readable ZIP.".into())
}

fn extract_file(
    entry: &mut zip::read::ZipFile<'_, fs::File>,
    destination: &Path,
    max: u64,
) -> Result<()> {
    if entry.size() > max
        || entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return Err("An archive contains a link or an oversized file.".into());
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(file_error)?;
    let expected = entry.size();
    if io::copy(&mut entry.take(expected + 1), &mut output).map_err(file_error)? != expected {
        return Err("An archive entry had an unexpected decompressed size.".into());
    }
    Ok(())
}

fn extract_java(path: &Path, destination: &Path) -> Result<()> {
    let mut zip = archive(path)?;
    let mut total: u64 = 0;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|_| "The Java runtime archive is incomplete.")?;
        let full = entry
            .enclosed_name()
            .ok_or("An archive entry escaped the Java runtime directory.")?;
        let relative = full
            .strip_prefix(JAVA_ARCHIVE_DIR)
            .map_err(|_| "Unexpected Java runtime archive layout.")?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if entry.is_dir() {
            managed_dir(destination, relative)?;
        } else {
            safe_relative(relative)?;
            total = total.saturating_add(entry.size());
            if total > 320 * 1024 * 1024 {
                return Err("The extracted Java runtime exceeds its expected size.".into());
            }
            managed_dir(
                destination,
                relative.parent().ok_or("Missing Java archive directory.")?,
            )?;
            extract_file(&mut entry, &destination.join(relative), 160 * 1024 * 1024)?;
        }
    }
    if !destination.join("bin/javaw.exe").is_file() {
        return Err("The Java runtime archive did not contain javaw.exe.".into());
    }
    Ok(())
}

async fn java(client: &Client, cache: &Path) -> Result<PathBuf> {
    let runtimes = managed_dir(cache, Path::new("runtimes"))?;
    let runtime = runtimes.join(JAVA_DIR);
    let executable = runtime.join("bin/javaw.exe");
    if runtime.exists() {
        let canonical_runtime = managed_dir(&runtimes, Path::new(JAVA_DIR))?;
        managed_dir(&canonical_runtime, Path::new("bin"))?;
        if fs::symlink_metadata(&executable).is_ok_and(|file| file.is_file() && ordinary(&file)) {
            return Ok(executable);
        }
        return Err("The cached Java runtime is incomplete. Remove its managed runtime directory and retry.".into());
    }
    let zip = fetch(
        client,
        cache,
        Download {
            url: JAVA_URL.into(),
            hash: JAVA_DIGEST.into(),
            size: None,
            relative: PathBuf::from(format!("runtimes/{JAVA_DIR}.zip")),
        },
    )
    .await?;
    let stage = runtimes.join(format!("java-{}", Uuid::new_v4()));
    fs::create_dir(&stage).map_err(file_error)?;
    let extracted = {
        let stage = stage.clone();
        tokio::task::spawn_blocking(move || extract_java(&zip, &stage))
            .await
            .map_err(|_| "The Java extraction task stopped.".to_string())
            .and_then(|result| result)
    };
    if let Err(error) = extracted.and_then(|_| fs::rename(&stage, &runtime).map_err(file_error)) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    Ok(executable)
}

fn natives(paths: &[PathBuf], destination: &Path) -> Result<()> {
    let mut written = HashSet::new();
    let mut total: u64 = 0;
    for path in paths {
        let mut zip = archive(path)?;
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|_| "A native library archive is incomplete.")?;
            let relative = entry
                .enclosed_name()
                .ok_or("A native library archive escaped its destination.")?;
            if entry.is_dir() || relative.extension().and_then(|name| name.to_str()) != Some("dll")
            {
                continue;
            }
            let name = relative
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("Invalid native library name.")?;
            let name_path = single_component(name)?;
            if !written.insert(name.to_ascii_lowercase()) {
                return Err("Two downloaded native libraries use the same filename.".into());
            }
            total = total.saturating_add(entry.size());
            if total > 128 * 1024 * 1024 {
                return Err("The extracted native libraries exceed their expected size.".into());
            }
            extract_file(&mut entry, &destination.join(name_path), 32 * 1024 * 1024)?;
        }
    }
    if written.is_empty() {
        return Err("The Minecraft metadata did not provide Windows x64 natives.".into());
    }
    Ok(())
}

async fn prepare_natives(
    cache: &Path,
    game_dir: &Path,
    archives: &[PathBuf],
    downloads: &[Download],
) -> Result<PathBuf> {
    let mut digest = Sha256::new();
    for archive in archives {
        let job = downloads
            .iter()
            .find(|job| job.relative == *archive)
            .ok_or("A native library was omitted from the download plan.")?;
        digest.update(job.hash.as_bytes());
    }
    let key = format!("{:x}", digest.finalize());
    let root = managed_dir(game_dir, Path::new(".bezalel/natives"))?;
    let destination = root.join(&key);
    if destination.exists() {
        return managed_dir(&root, Path::new(&key));
    }
    let stage = root.join(Uuid::new_v4().to_string());
    fs::create_dir(&stage).map_err(file_error)?;
    let result = {
        let paths: Vec<_> = archives.iter().map(|path| cache.join(path)).collect();
        let stage = stage.clone();
        tokio::task::spawn_blocking(move || natives(&paths, &stage))
            .await
            .map_err(|_| "The native-library extraction task stopped.".to_string())
            .and_then(|result| result)
    };
    if let Err(error) = result.and_then(|_| fs::rename(&stage, &destination).map_err(file_error)) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    Ok(destination)
}

fn maven_path(coordinate: &str) -> Result<PathBuf> {
    let fields: Vec<_> = coordinate.split(':').collect();
    let [group, artifact, version, optional @ ..] = fields.as_slice() else {
        return Err("Fabric supplied an invalid Maven coordinate.".into());
    };
    if optional.len() > 1
        || fields.iter().any(|value| {
            value.is_empty()
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
        })
    {
        return Err("Fabric supplied an unsafe Maven coordinate.".into());
    }
    let group = group.replace('.', "/");
    let classifier = optional
        .first()
        .map(|value| format!("-{value}"))
        .unwrap_or_default();
    let path = PathBuf::from(format!(
        "{group}/{artifact}/{version}/{artifact}-{version}{classifier}.jar"
    ));
    safe_relative(&path)?;
    Ok(path)
}

async fn fabric_library(client: &Client, library: FabricLibrary) -> Result<Download> {
    let path = maven_path(&library.name)?;
    let portable = path
        .to_str()
        .ok_or("Invalid Fabric library path.")?
        .replace('\\', "/");
    let base = Url::parse(&library.url).map_err(|_| "Fabric supplied an invalid library URL.")?;
    if !official_url(&base)
        || !matches!(
            base.host_str(),
            Some("maven.fabricmc.net" | "libraries.minecraft.net")
        )
    {
        return Err("Fabric supplied a library on an unapproved Maven host.".into());
    }
    let url = base
        .join(&portable)
        .map_err(|_| "Fabric supplied an invalid library location.")?
        .to_string();
    let hash = match library.sha256.or(library.sha1) {
        Some(hash) => hash,
        None => {
            let response = metadata(client, &format!("{url}.sha1"), 256).await?;
            let text = std::str::from_utf8(&response)
                .map_err(|_| "Fabric supplied an unreadable Maven checksum.")?;
            text.split_whitespace()
                .next()
                .ok_or("Fabric did not supply a Maven checksum.")?
                .to_string()
        }
    };
    Ok(Download {
        url,
        hash,
        size: library.size,
        relative: PathBuf::from("libraries").join(path),
    })
}

fn library_file(file: &Artifact) -> Result<PathBuf> {
    let path = PathBuf::from(
        file.path
            .as_deref()
            .ok_or("Minecraft supplied a library without a path.")?,
    );
    safe_relative(&path)?;
    Ok(PathBuf::from("libraries").join(path))
}

type LibraryPaths = Vec<(String, PathBuf)>;

fn game_libraries(libraries: Vec<Library>) -> Result<(Vec<Download>, LibraryPaths, Vec<PathBuf>)> {
    let mut jobs = Vec::new();
    let mut classpath = Vec::new();
    let mut native = Vec::new();
    for library in libraries {
        if !allowed(&library.rules, false)?
            || library.name.ends_with(":natives-windows-x86")
            || library.name.ends_with(":natives-windows-arm64")
        {
            continue;
        }
        if let Some(file) = library.downloads.artifact {
            let path = library_file(&file)?;
            if library.name.ends_with(":natives-windows") {
                native.push(path.clone());
            }
            classpath.push((library.name.clone(), path.clone()));
            jobs.push(Download::official(file, path));
        }
        if let Some(classifier) = library.natives.get("windows") {
            let classifier = classifier.replace("${arch}", "64");
            let file = library
                .downloads
                .classifiers
                .get(&classifier)
                .ok_or("Minecraft did not supply a Windows library classifier.")?
                .clone();
            let path = library_file(&file)?;
            native.push(path.clone());
            jobs.push(Download::official(file, path));
        }
    }
    Ok((jobs, classpath, native))
}

fn library_identity(coordinate: &str) -> (&str, &str, Option<&str>) {
    let mut fields = coordinate.split(':');
    let group = fields.next().unwrap_or_default();
    let artifact = fields.next().unwrap_or_default();
    let _version = fields.next();
    (group, artifact, fields.next())
}

async fn prepare_fabric(
    client: &Client,
    cache: &Path,
    target: Target,
    emit: &impl Fn(String),
) -> Result<installer::Prepared> {
    let url = format!(
        "https://meta.fabricmc.net/v2/versions/loader/{}/{}/profile/json",
        target.minecraft_version, target.loader_version,
    );
    let profile: FabricProfile =
        serde_json::from_slice(&metadata(client, &url, MAX_METADATA).await?)
            .map_err(|_| "The Fabric profile uses an unsupported format.")?;
    if profile.inherits_from != target.minecraft_version
        || profile.id
            != format!(
                "fabric-loader-{}-{}",
                target.loader_version, target.minecraft_version
            )
        || profile.main_class != "net.fabricmc.loader.impl.launch.knot.KnotClient"
    {
        return Err("Fabric returned a profile that does not match the pinned loader.".into());
    }
    let mut jobs = Vec::new();
    let mut libraries = Vec::new();
    for library in profile.libraries {
        let name = library.name.clone();
        let job = fabric_library(client, library).await?;
        libraries.push((name, cache.join(&job.relative)));
        jobs.push(job);
    }
    fetch_all(client, cache, &jobs, emit, "Verifying Fabric libraries").await?;
    Ok(installer::Prepared {
        id: profile.id,
        main_class: profile.main_class,
        arguments: profile.arguments,
        libraries,
        directory: cache.join("libraries"),
    })
}

pub async fn launch(
    root: &Path,
    game_dir: &Path,
    target: Target,
    profile: &Profile,
    memory_mib: u32,
    address: &str,
    emit: impl Fn(String) + Send + Sync,
) -> Result<Child> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return Err("Native Minecraft launch currently requires Windows x64.".into());
    }
    if !(1024..=32768).contains(&memory_mib) {
        return Err("Choose between 1024 and 32768 MiB of memory in Settings.".into());
    }
    let address = address.trim();
    if address.len() > 255
        || address.starts_with('-')
        || address.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("The pack has an invalid server address.".into());
    }
    let _install = INSTALL.lock().await;
    fs::create_dir_all(root).map_err(file_error)?;
    fs::create_dir_all(game_dir).map_err(file_error)?;
    let cache = managed_dir(root, Path::new("minecraft-cache"))?;
    let client = download_client()?;

    emit(format!("Resolving Minecraft and {}", target.loader));
    let catalog: Catalog =
        serde_json::from_slice(&metadata(&client, MANIFEST, MAX_METADATA).await?)
            .map_err(|_| "The Minecraft version catalog uses an unsupported format.")?;
    let version = catalog
        .versions
        .into_iter()
        .find(|version| version.id == target.minecraft_version)
        .ok_or("The pinned Minecraft version is missing from Mojang's catalog.")?;
    let game_metadata = fetch(
        &client,
        &cache,
        Download {
            url: version.url,
            hash: version.sha1,
            size: None,
            relative: PathBuf::from(format!("versions/{0}/{0}.json", target.minecraft_version)),
        },
    )
    .await?;
    let game: Game = file_json(&game_metadata)?;
    emit("Checking Java 21".into());
    let java = java(&client, &cache).await?;
    let client_path = PathBuf::from(format!("versions/{0}/{0}.jar", target.minecraft_version));
    let (mut libraries, vanilla_classpath, native_paths) = game_libraries(game.libraries)?;
    libraries.push(Download::official(
        game.downloads.client,
        client_path.clone(),
    ));
    fetch_all(&client, &cache, &libraries, &emit, "Verifying game files").await?;
    let client_jar = cache.join(client_path);
    let loader = match target.loader {
        "fabric" => prepare_fabric(&client, &cache, target, &emit).await?,
        "forge" | "neoforge" => {
            installer::prepare(
                &client,
                &cache,
                &java,
                target,
                &game_metadata,
                &client_jar,
                &emit,
            )
            .await?
        }
        _ => return Err("This target does not have a native launcher.".into()),
    };
    let overrides: HashSet<_> = loader
        .libraries
        .iter()
        .map(|(name, _)| library_identity(name))
        .collect();
    let mut paths: Vec<_> = loader
        .libraries
        .iter()
        .map(|(_, path)| path.clone())
        .collect();
    paths.extend(vanilla_classpath.into_iter().filter_map(|(name, path)| {
        (!overrides.contains(&library_identity(&name))).then(|| cache.join(path))
    }));
    paths.push(client_jar);

    let asset_index_name = single_component(&format!("{}.json", game.asset_index.id))?;
    let asset_index_path = fetch(
        &client,
        &cache,
        Download::official(
            game.asset_index.file,
            PathBuf::from("assets/indexes").join(asset_index_name),
        ),
    )
    .await?;
    let assets: Assets = file_json(&asset_index_path)?;
    let mut objects = Vec::new();
    let mut object_hashes = HashSet::new();
    for asset in assets.objects.into_values() {
        if asset.hash.len() != 40 || !asset.hash.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("The Minecraft asset index contains an invalid SHA-1.".into());
        }
        if object_hashes.insert(asset.hash.clone()) {
            let shard = &asset.hash[..2];
            objects.push(Download {
                url: format!(
                    "https://resources.download.minecraft.net/{shard}/{}",
                    asset.hash
                ),
                hash: asset.hash.clone(),
                size: Some(asset.size),
                relative: PathBuf::from(format!("assets/objects/{shard}/{}", asset.hash)),
            });
        }
    }
    fetch_all(
        &client,
        &cache,
        &objects,
        &emit,
        "Verifying Minecraft assets",
    )
    .await?;

    let logging = match game.logging {
        Some(logging) => {
            let name = single_component(&logging.client.file.id)?;
            let path = fetch(
                &client,
                &cache,
                Download::official(
                    logging.client.file.file,
                    PathBuf::from("assets/log_configs").join(name),
                ),
            )
            .await?;
            if logging.client.argument != "-Dlog4j.configurationFile=${path}" {
                return Err(
                    "Minecraft returned an unrecognized logging configuration argument.".into(),
                );
            }
            Some(format!(
                "-Dlog4j.configurationFile={}",
                path.to_string_lossy()
            ))
        }
        None => None,
    };

    emit("Preparing Windows native libraries".into());
    let natives_dir = prepare_natives(&cache, game_dir, &native_paths, &libraries).await?;
    let quick_play = !address.is_empty();
    let quick_play_path = managed_dir(game_dir, Path::new(".bezalel"))?.join("quick-play.json");
    let classpath = std::env::join_paths(paths)
        .map_err(|_| "The Minecraft library path contains a semicolon.")?
        .into_string()
        .map_err(|_| "A Minecraft library path contains invalid Unicode.")?;
    let mut values = BTreeMap::from([
        ("version_name", loader.id),
        ("version_type", game.version_type),
        ("user_type", "msa".into()),
        ("game_directory", game_dir.to_string_lossy().into_owned()),
        (
            "assets_root",
            cache.join("assets").to_string_lossy().into_owned(),
        ),
        ("assets_index_name", game.asset_index.id),
        (
            "natives_directory",
            natives_dir.to_string_lossy().into_owned(),
        ),
        (
            "library_directory",
            loader.directory.to_string_lossy().into_owned(),
        ),
        ("classpath", classpath),
        ("classpath_separator", ";".into()),
        ("launcher_name", "Bezalel".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
        (
            "quickPlayPath",
            quick_play_path.to_string_lossy().into_owned(),
        ),
        ("quickPlayMultiplayer", address.into()),
    ]);
    let mut vm = launch_arguments(&game.arguments.jvm, quick_play)?;
    vm.extend(launch_arguments(&loader.arguments.jvm, quick_play)?);
    let mut game = launch_arguments(&game.arguments.game, quick_play)?;
    game.extend(launch_arguments(&loader.arguments.game, quick_play)?);
    let mut command = Command::new(java);
    let mut java_options = vec![format!("-Xmx{memory_mib}M")];
    java_options.extend(
        vm.iter()
            .map(|argument| expand(argument, &values))
            .collect::<Result<Vec<_>>>()?,
    );
    if let Some(logging) = logging {
        java_options.push(logging);
    }
    if java_options
        .iter()
        .map(|value| value.encode_utf16().count() + 3)
        .sum::<usize>()
        > 23_000
    {
        // Keep the long classpath out of the Windows command line. This file
        // contains JVM options only, before any account values are available.
        let argfile = managed_dir(game_dir, Path::new(".bezalel"))?.join("jvm-args.txt");
        crate::studio::write_atomic(&argfile, java_argfile(&java_options)?.as_bytes())?;
        command.arg("@.bezalel/jvm-args.txt");
    } else {
        command.args(java_options);
    }
    command.arg("--disable-@files");
    values.extend([
        ("auth_player_name", profile.name.clone()),
        ("auth_uuid", profile.id.clone()),
        ("auth_access_token", profile.access_token.clone()),
        ("auth_xuid", profile.xuid.clone()),
        ("clientid", profile.client_id.clone()),
    ]);
    command.arg(loader.main_class);
    for argument in game {
        command.arg(expand(&argument, &values)?);
    }
    // JVM option environment variables could inject code before Minecraft starts.
    command
        .env_remove("JAVA_TOOL_OPTIONS")
        .env_remove("JDK_JAVA_OPTIONS")
        .env_remove("_JAVA_OPTIONS");
    command
        .current_dir(game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    emit("Starting Minecraft".into());
    command.spawn().map_err(|_| {
        "Minecraft did not start. Check the verified Java runtime and game directory.".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_reject_redirecting_names() {
        assert!(safe_relative(Path::new("libraries/org.example/a-1.0.jar")).is_ok());
        for path in ["../outside", "libraries/CON.jar", "test:stream", "test."] {
            assert!(safe_relative(Path::new(path)).is_err(), "{path}");
        }
    }

    #[test]
    fn quick_play_uses_rules_and_keeps_argument_values_intact() {
        let arguments: Vec<Argument> = serde_json::from_value(serde_json::json!([
            {"rules": [{"action": "allow", "features": {"is_quick_play_multiplayer": true}}],
             "value": ["--quickPlayMultiplayer", "${quickPlayMultiplayer}"]}
        ]))
        .unwrap();
        assert!(launch_arguments(&arguments, false).unwrap().is_empty());
        let arguments = launch_arguments(&arguments, true).unwrap();
        assert_eq!(arguments[0], "--quickPlayMultiplayer");
        let values = BTreeMap::from([("quickPlayMultiplayer", "[::1]:25565".into())]);
        assert_eq!(expand(&arguments[1], &values).unwrap(), "[::1]:25565");
    }
}
