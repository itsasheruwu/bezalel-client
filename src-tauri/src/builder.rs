use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};
use uuid::Uuid;

const MAX_SOURCE: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 1_024;
const MAX_JAR: usize = 24 * 1024 * 1024;
const MAX_LOG: usize = 128 * 1024;
const PODMAN_TIMEOUT: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub jar_path: PathBuf,
    pub sha256: String,
    pub size: usize,
    pub compile_passed: bool,
    pub client_passed: bool,
    pub server_passed: bool,
    pub diagnostics: Vec<String>,
}

struct Target {
    image: &'static str,
    metadata: &'static str,
    json_metadata: bool,
}

fn target(loader: &str, version: &str) -> Result<Target, String> {
    if version != "1.21.1" {
        return Err("The build runner currently supports Minecraft 1.21.1.".into());
    }
    let (image, metadata, json_metadata) = match loader {
        "fabric" => ("fabric", "fabric.mod.json", true),
        "forge" => ("forge", "META-INF/mods.toml", false),
        "neoforge" => ("neoforge", "META-INF/neoforge.mods.toml", false),
        _ => return Err("Unknown mod loader.".into()),
    };
    let image = match image {
        "fabric" => "localhost/bezalel-builder:fabric-1.21.1-v1",
        "forge" => "localhost/bezalel-builder:forge-1.21.1-v2",
        _ => "localhost/bezalel-builder:neoforge-1.21.1-v1",
    };
    Ok(Target {
        image,
        metadata,
        json_metadata,
    })
}

fn ordinary(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions are reparse points even when is_symlink() reports false.
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !metadata.file_type().is_symlink()
}

fn safe_component(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with('.')
        || value.ends_with('.')
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
    {
        return false;
    }
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

fn supported_file(relative: &str) -> bool {
    if relative.starts_with("src/main/java/") {
        return relative.ends_with(".java");
    }
    let Some(resource) = relative.strip_prefix("src/main/resources/") else {
        return false;
    };
    if resource.to_ascii_lowercase().starts_with("meta-inf/") {
        return matches!(
            resource,
            "META-INF/mods.toml" | "META-INF/neoforge.mods.toml" | "META-INF/accesstransformer.cfg"
        );
    }
    matches!(
        resource.rsplit_once('.').map(|(_, extension)| extension),
        Some(
            "json"
                | "toml"
                | "mcmeta"
                | "png"
                | "ogg"
                | "lang"
                | "txt"
                | "md"
                | "nbt"
                | "mcfunction"
                | "vsh"
                | "fsh"
                | "glsl"
                | "properties"
                | "cfg"
                | "accesswidener"
        )
    )
}

fn collect_source(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
    paths: &mut HashSet<String>,
    remaining: &mut usize,
) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|_| "Invalid draft path.")?;
        let components: Option<Vec<_>> = relative
            .iter()
            .map(|name| name.to_str().filter(|name| safe_component(name)))
            .collect();
        let relative = components
            .ok_or("The draft contains an invalid source path.")?
            .join("/");
        if relative.len() > 240
            || paths.len() >= MAX_FILES
            || !paths.insert(relative.to_ascii_lowercase())
        {
            return Err("The draft contains too many, oversized or duplicate source paths.".into());
        }
        let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
        if !ordinary(&metadata) || !fs::canonicalize(&path).map_err(io_error)?.starts_with(root) {
            return Err("The draft contains a link or redirected source path.".into());
        }
        if metadata.is_dir() {
            collect_source(root, &path, files, paths, remaining)?;
            continue;
        }
        if !metadata.is_file() || !supported_file(&relative) {
            return Err(format!(
                "Unsupported draft file {relative}. Use Java or mod resources."
            ));
        }
        let mut file = fs::OpenOptions::new();
        file.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            file.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        }
        let file = file.open(&path).map_err(io_error)?;
        if !ordinary(&file.metadata().map_err(io_error)?) {
            return Err("The draft contains a redirected source file.".into());
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(file, (*remaining + 1) as u64),
            &mut bytes,
        )
        .map_err(io_error)?;
        *remaining = remaining
            .checked_sub(bytes.len())
            .ok_or("Draft source exceeds 16 MiB.")?;
        files.push((relative, bytes));
    }
    Ok(())
}

fn parse_mod_id(target: &Target, bytes: &[u8]) -> Result<String, String> {
    let invalid = || format!("{} needs one valid mod ID.", target.metadata);
    let id = if target.json_metadata {
        let metadata: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        metadata["id"].as_str().map(str::to_owned)
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
        let metadata: toml::Value = toml::from_str(text).map_err(|_| invalid())?;
        metadata
            .get("mods")
            .and_then(toml::Value::as_array)
            .filter(|mods| mods.len() == 1)
            .and_then(|mods| mods[0].get("modId"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    }
    .ok_or_else(invalid)?;
    if !(2..=64).contains(&id.len())
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        || !safe_component(&id)
    {
        return Err(invalid());
    }
    Ok(id)
}

fn validate_jar(bytes: &[u8], target: &Target, expected_id: &str) -> Result<(), String> {
    use std::io::Read;

    let mut jar = zip::ZipArchive::new(io::Cursor::new(bytes))
        .map_err(|_| "The isolated runner did not return a readable JAR.")?;
    if !jar.file_names().any(|name| name.ends_with(".class")) {
        return Err("The generated JAR does not contain compiled Java classes.".into());
    }
    let metadata = jar
        .by_name(target.metadata)
        .map_err(|_| "The generated JAR does not contain its loader metadata.")?;
    if metadata.size() > 128 * 1024 {
        return Err("The generated JAR contains oversized loader metadata.".into());
    }
    let mut bytes = Vec::new();
    metadata
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if parse_mod_id(target, &bytes)? != expected_id {
        return Err("The generated JAR has an unexpected mod ID.".into());
    }
    Ok(())
}

fn snapshot(draft: &Path, target: &Target) -> Result<(String, Vec<u8>), String> {
    let root = fs::canonicalize(draft).map_err(io_error)?;
    let source = root.join("src");
    let metadata = fs::symlink_metadata(&source)
        .map_err(|_| "Generate mod source before building.".to_string())?;
    if !ordinary(&metadata) || !metadata.is_dir() {
        return Err("The draft source must be a regular directory.".into());
    }
    let mut files = Vec::new();
    let mut paths = HashSet::new();
    let mut remaining = MAX_SOURCE;
    collect_source(&root, &source, &mut files, &mut paths, &mut remaining)?;
    if !files.iter().any(|(path, _)| path.ends_with(".java")) {
        return Err("The draft does not contain Java source yet.".into());
    }
    let expected = format!("src/main/resources/{}", target.metadata);
    let bytes = files
        .iter()
        .find_map(|(path, bytes)| (path == &expected).then_some(bytes))
        .ok_or_else(|| format!("The draft is missing {}.", target.metadata))?;
    let mod_id = parse_mod_id(target, bytes)?;

    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut archive = tar::Builder::new(Vec::new());
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, path, bytes.as_slice())
            .map_err(io_error)?;
    }
    Ok((mod_id, archive.into_inner().map_err(io_error)?))
}

fn io_error(error: io::Error) -> String {
    format!("Builder file or process error ({:?}).", error.kind())
}

pub fn local_podman_environment(command: &mut Command) {
    command
        .env_remove("CONTAINER_HOST")
        .env_remove("CONTAINER_CONNECTION")
        .env_remove("CONTAINER_SSHKEY")
        .env_remove("DOCKER_HOST")
        .env_remove("CONTAINERS_CONF")
        .env_remove("CONTAINERS_CONF_OVERRIDE")
        .env_remove("CONTAINERS_CONF_MODULES")
        .env_remove("CONTAINERS_STORAGE_CONF")
        .env_remove("CONTAINERS_REGISTRIES_CONF")
        .env_remove("PODMAN_CONNECTIONS_CONF")
        .env_remove("CONTAINERS_MACHINE_PROVIDER");
}

#[cfg(windows)]
fn podman_executable() -> Result<PathBuf, String> {
    let installed = [
        ("LOCALAPPDATA", r"Programs\Podman\podman.exe"),
        ("LOCALAPPDATA", r"Programs\RedHat\Podman\podman.exe"),
        ("ProgramFiles", r"Podman\podman.exe"),
        ("ProgramFiles", r"RedHat\Podman\podman.exe"),
    ];
    let candidates = installed.into_iter().filter_map(|(variable, relative)| {
        let directory = PathBuf::from(std::env::var_os(variable)?);
        directory.is_absolute().then(|| directory.join(relative))
    });
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path_candidates = std::env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join("podman.exe"));
    candidates
        .chain(path_candidates)
        .find(|path| path.is_file())
        .ok_or_else(|| "The local container runner is unavailable.".into())
}

fn podman() -> Result<Command, String> {
    #[cfg(windows)]
    let mut command = Command::new(podman_executable()?);
    #[cfg(not(windows))]
    let mut command = Command::new("podman");
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    local_podman_environment(&mut command);
    command.kill_on_drop(true);
    Ok(command)
}

fn engine() -> Result<Command, String> {
    let mut command = podman()?;
    #[cfg(windows)]
    command.args(["--connection", "bezalel-builder"]);
    Ok(command)
}

async fn probe(mut command: Command) -> Result<Vec<u8>, String> {
    let output = timeout(Duration::from_secs(20), command.output())
        .await
        .map_err(|_| {
            "Podman did not respond. Check that its local machine is running.".to_string()
        })?
        .map_err(|_| "Bezalel could not start the local container runner.")?;
    if !output.status.success() {
        return Err("The local Podman engine or builder image is unavailable.".into());
    }
    Ok(output.stdout)
}

async fn image_id(target: &Target) -> Result<String, String> {
    #[cfg(windows)]
    {
        let mut command = podman()?;
        command.args(["system", "connection", "list", "--format", "json"]);
        let result = probe(command).await?;
        let connections: Vec<serde_json::Value> = serde_json::from_slice(&result)
            .map_err(|_| "Podman returned an unreadable connection list.")?;
        let local = connections.iter().any(|entry| {
            entry["Name"] == "bezalel-builder"
                && entry["IsMachine"] == true
                && entry["URI"]
                    .as_str()
                    .and_then(|uri| uri.strip_prefix("ssh://"))
                    .and_then(|uri| uri.split_once('@').map(|(_, host)| host))
                    .is_some_and(|host| {
                        host.starts_with("127.0.0.1:") || host.starts_with("[::1]:")
                    })
        });
        if !local {
            return Err(
                "The builder requires the local rootless bezalel-builder connection.".into(),
            );
        }
    }

    let mut command = engine()?;
    command.args(["info", "--format", "json"]);
    let result = probe(command).await?;
    let info: serde_json::Value = serde_json::from_slice(&result)
        .map_err(|_| "Podman returned unreadable engine information.")?;
    let host = &info["host"];
    if host["os"] != "linux"
        || host["cgroupVersion"] != "v2"
        || host["security"]["rootless"] != true
        || host["security"]["seccompEnabled"] != true
    {
        return Err(
            "The builder requires rootless Linux Podman with cgroups v2 and seccomp.".into(),
        );
    }
    if !host["cgroupControllers"]
        .as_array()
        .is_some_and(|controllers| {
            ["cpu", "memory", "pids"].iter().all(|required| {
                controllers
                    .iter()
                    .any(|value| value.as_str() == Some(required))
            })
        })
    {
        return Err("The local builder cannot enforce its CPU, memory and process limits.".into());
    }

    let mut command = engine()?;
    command.args(["image", "inspect", target.image]);
    let result = probe(command).await.map_err(|_| {
        format!(
            "Prepare the local image {} using builder/README.md.",
            target.image
        )
    })?;
    let images: Vec<serde_json::Value> = serde_json::from_slice(&result)
        .map_err(|_| "Podman returned unreadable builder image information.")?;
    let Some(image) = images.first() else {
        return Err("The requested builder image is missing.".into());
    };
    if image["Labels"]["org.bezalel.minecraft-eula"] != "accepted" {
        return Err("Prepare the builder image after accepting the Minecraft EULA.".into());
    }
    let id = image["Id"].as_str().unwrap_or("");
    let digest = id.strip_prefix("sha256:").unwrap_or(id);
    if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("The builder image does not have a valid local ID.".into());
    }
    Ok(id.to_owned())
}

pub async fn check_environment(loader: &str, version: &str) -> Result<(), String> {
    image_id(&target(loader, version)?).await.map(|_| ())
}

async fn bounded_output<R: AsyncRead + Unpin>(reader: R, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err("The isolated build exceeded its output limit.".into());
    }
    Ok(bytes)
}

fn diagnostic(log: &[u8]) -> String {
    let text = String::from_utf8_lossy(log);
    let tail = text.lines().rev().take(18).collect::<Vec<_>>();
    let clean = tail
        .into_iter()
        .rev()
        .flat_map(|line| line.chars().chain(std::iter::once('\n')))
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(3_000)
        .collect::<String>();
    clean.trim().to_string()
}

async fn run_step(
    image: &str,
    step: &str,
    mod_id: &str,
    snapshot: &[u8],
) -> Result<Vec<u8>, String> {
    let name = format!("bezalel-{}", Uuid::new_v4().simple());
    let mut command = engine()?;
    command.args([
        "run",
        "--rm",
        "--interactive",
        "--name",
        &name,
        "--pull=never",
        "--network=none",
        "--http-proxy=false",
        "--read-only",
        "--cap-drop=all",
        "--security-opt=no-new-privileges",
        "--user=1000:1000",
        "--cgroupns=private",
        "--pids-limit=256",
        "--memory=6g",
        "--memory-swap=6g",
        "--cpus=2",
        "--image-volume=ignore",
        "--log-driver=none",
        "--timeout",
        &PODMAN_TIMEOUT.to_string(),
        "--tmpfs",
        "/work:rw,exec,nosuid,nodev,mode=1777,size=4g",
        "--entrypoint",
        "/usr/local/bin/bezalel-build",
        image,
        step,
        mod_id,
    ]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let result = timeout(Duration::from_secs(PODMAN_TIMEOUT + 15), async {
        let send = async {
            // An early container error can close stdin. Keep its stderr for diagnosis.
            let _ = stdin.write_all(snapshot).await;
            drop(stdin);
            Ok(())
        };
        tokio::try_join!(
            send,
            bounded_output(stdout, if step == "compile" { MAX_JAR } else { 128 }),
            bounded_output(stderr, MAX_LOG),
            async { child.wait().await.map_err(io_error) },
        )
    })
    .await;
    match result {
        Ok(Ok((_, _, log, status))) if !status.success() => {
            let log = diagnostic(&log);
            let summary = format!("{step} exited with status {status}");
            if log.is_empty() {
                Err(summary)
            } else {
                Err(format!("{summary}\n{log}"))
            }
        }
        Ok(Ok((_, output, _, _))) => Ok(output),
        failed => {
            drop(child);
            if let Ok(mut cleanup) = engine() {
                cleanup.args(["rm", "--force", "--ignore", &name]);
                let _ = timeout(Duration::from_secs(10), cleanup.output()).await;
            }
            match failed {
                Ok(Err(error)) => Err(error),
                _ => Err(format!("{step} timed out in the isolated runner.")),
            }
        }
    }
}

pub async fn build(
    root: &Path,
    _resources: &Path,
    draft_id: &str,
    loader: &str,
    version: &str,
    emit: impl Fn(String) + Send + Sync,
) -> Result<BuildResult, String> {
    let target = target(loader, version)?;
    if !safe_component(draft_id) {
        return Err("Invalid draft ID.".into());
    }
    let draft = root.join("drafts").join(draft_id);
    for directory in [root.join("drafts"), draft.clone()] {
        let metadata = fs::symlink_metadata(&directory).map_err(io_error)?;
        if !metadata.is_dir() || !ordinary(&metadata) {
            return Err("The draft must be stored in a regular app directory.".into());
        }
    }
    emit("Validating mod source".into());
    let (mod_id, archive) = snapshot(&draft, &target)?;
    let image = image_id(&target).await?;
    emit("Compiling mod offline".into());
    let jar = run_step(&image, "compile", &mod_id, &archive).await?;
    validate_jar(&jar, &target, &mod_id)?;
    let mut diagnostics = Vec::new();
    let mut smoke = [false; 2];
    for (index, step) in ["client", "server"].iter().enumerate() {
        emit(format!("Checking {step} startup in isolation"));
        match run_step(&image, step, &mod_id, &archive).await {
            Ok(output) if output == b"passed\n" => smoke[index] = true,
            Ok(_) => diagnostics.push(format!("{step} startup did not return a valid result.")),
            Err(error) => diagnostics.push(error),
        }
    }
    let builds = draft.join("builds");
    match fs::create_dir(&builds) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&builds).map_err(io_error)?;
            if !metadata.is_dir() || !ordinary(&metadata) {
                return Err("The build output must be a regular app directory.".into());
            }
        }
        Err(error) => return Err(io_error(error)),
    }
    let output = builds.join(Uuid::new_v4().to_string());
    fs::create_dir(&output).map_err(io_error)?;
    let jar_path = output.join(format!("{mod_id}.jar"));
    fs::write(&jar_path, &jar).map_err(io_error)?;
    Ok(BuildResult {
        jar_path,
        sha256: format!("{:x}", Sha256::digest(&jar)),
        size: jar.len(),
        compile_passed: true,
        client_passed: smoke[0],
        server_passed: smoke[1],
        diagnostics,
    })
}
