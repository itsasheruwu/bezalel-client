use std::path::Path;

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Ready,
    ConsentRequired,
}

pub fn gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn complete_builder(root: &Path) -> bool {
    [
        "setup-windows.ps1",
        "prepare.ps1",
        "Containerfile",
        "run.sh",
        "smoke.sh",
    ]
    .iter()
    .all(|name| root.join("builder").join(name).is_file())
        && root.join("builder/templates").is_dir()
        && root.join("builder/fixtures").is_dir()
}

fn resolve_resource_root(
    packaged: &Path,
    development: Option<&Path>,
) -> Result<std::path::PathBuf, String> {
    if complete_builder(packaged) {
        return Ok(packaged.to_owned());
    }
    if let Some(root) = development.filter(|root| complete_builder(root)) {
        return Ok(root.to_owned());
    }
    Err("The builder resources are incomplete. Rebuild or reinstall Bezalel with its builder files.".into())
}

fn resource_root(packaged: &Path) -> Result<std::path::PathBuf, String> {
    // Cargo-run development executables may live outside Tauri's bundle
    // directory. Never search the current working directory for scripts.
    let development = if cfg!(debug_assertions) {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent()
    } else {
        None
    };
    resolve_resource_root(packaged, development)
}

#[cfg(windows)]
fn powershell_path(path: &Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf, Prefix};

    let mut components = path.components();
    if let Some(Component::Prefix(prefix)) = components.next() {
        if let Prefix::VerbatimDisk(drive) = prefix.kind() {
            if components.next() == Some(Component::RootDir) {
                // Windows PowerShell 5.1 treats a verbatim drive path as remote
                // under RemoteSigned, even when the file is installed locally.
                let mut local = PathBuf::from(format!("{}:\\", char::from(drive)));
                local.extend(components);
                return local;
            }
        }
    }
    path.to_owned()
}

#[cfg(windows)]
pub async fn prepare(
    resources: &Path,
    state_directory: &Path,
    loader: &str,
    version: &str,
    accepted: bool,
    progress: impl FnMut(String),
) -> Result<Outcome, String> {
    use std::{path::PathBuf, process::Stdio, time::Duration};
    use tokio::{process::Command, time::timeout};

    let resources = powershell_path(&resource_root(resources)?);
    let state_directory = powershell_path(state_directory);
    let script = resources.join("builder").join("setup-windows.ps1");
    let powershell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
        .filter(|path| path.is_file())
        .ok_or("Bezalel could not find Windows PowerShell for builder setup.")?;
    crate::state::ensure_directory(&state_directory)?;
    let mut command = Command::new(powershell);
    command
        .creation_flags(0x0800_0000)
        .current_dir(resources)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "RemoteSigned",
            "-File",
        ])
        .arg(script)
        .args([
            "-Loader",
            loader,
            "-MinecraftVersion",
            version,
            "-StateDirectory",
        ])
        .arg(state_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if accepted {
        command.arg("-AcceptMinecraftEula");
    }
    crate::builder::local_podman_environment(&mut command);
    let mut child = command
        .spawn()
        .map_err(|_| "Bezalel could not start the background builder setup.")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Builder setup progress is unavailable.")?;
    let status = timeout(Duration::from_secs(45 * 60), async {
        let (status, ()) = tokio::try_join!(child.wait(), report_progress(stdout, progress))?;
        Ok::<_, std::io::Error>(status)
    })
    .await
    .map_err(|_| "Builder setup timed out. Check your connection and try Build mod again.")?
    .map_err(|_| "Bezalel lost contact with background builder setup.")?;
    setup_outcome(status.code(), accepted)
}

#[cfg(not(windows))]
pub async fn prepare(
    _: &Path,
    _: &Path,
    _: &str,
    _: &str,
    _: bool,
    _: impl FnMut(String),
) -> Result<Outcome, String> {
    Err("Automatic builder setup is available on Windows 11 x64.".into())
}

#[cfg(windows)]
async fn report_progress(
    stdout: impl tokio::io::AsyncRead + Unpin,
    mut progress: impl FnMut(String),
) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let mut reader = BufReader::new(stdout);
    let mut line = Vec::with_capacity(256);
    loop {
        line.clear();
        let count = (&mut reader).take(256).read_until(b'\n', &mut line).await?;
        if count == 0 {
            return Ok(());
        }
        if line.last() != Some(&b'\n') {
            loop {
                let buffered = reader.fill_buf().await?;
                if buffered.is_empty() {
                    return Ok(());
                }
                let newline = buffered.iter().position(|byte| *byte == b'\n');
                let consumed = newline.map_or(buffered.len(), |index| index + 1);
                reader.consume(consumed);
                if newline.is_some() {
                    break;
                }
            }
            continue;
        }
        if let Some(message) = std::str::from_utf8(&line).ok().and_then(stage_message) {
            progress(message.into());
        }
    }
}

#[cfg(any(windows, test))]
fn stage_message(line: &str) -> Option<&'static str> {
    match line.trim().strip_prefix("BEZALEL_SETUP ")? {
        "checking" => Some("Checking the local mod builder…"),
        "installing-wsl" => Some("Setting up the Windows builder. Approve the Windows prompt if it appears…"),
        "installing-podman" => Some("Downloading and verifying the container runner…"),
        "initializing-machine" => Some("Creating the isolated build environment…"),
        "starting-machine" => Some("Starting the isolated build environment…"),
        "repairing-cgroups" => Some("Configuring the build environment’s safety limits…"),
        "verifying-isolation" => Some("Checking the builder’s safety limits…"),
        "preparing-image" => Some("Preparing the Minecraft build tools and testing startup. This can take several minutes…"),
        "ready" => Some("The local mod builder is ready."),
        _ => None,
    }
}

#[cfg(any(windows, test))]
fn setup_outcome(code: Option<i32>, accepted: bool) -> Result<Outcome, String> {
    let message = match code {
        Some(0) => return Ok(Outcome::Ready),
        Some(10) if !accepted => return Ok(Outcome::ConsentRequired),
        Some(11) => "Choose a supported Minecraft target before setting up the builder.",
        Some(12) => "Automatic builder setup requires Windows x64.",
        Some(20) => "Windows could not enable the local builder. Approve the Windows prompt if it appears and try again.",
        Some(21) => "Restart Windows to finish setting up the local builder. Reopen Bezalel and click Build mod again.",
        Some(30) => "Bezalel could not install or find the container runner. Check your connection and try again.",
        Some(40) => "Bezalel could not start the isolated build environment. Restart Windows and try Build mod again.",
        Some(41) => "The Bezalel build environment could not be prepared. Retry mod tools setup.",
        Some(42) => "Bezalel could not verify the builder’s safety limits. Mod code was not run.",
        Some(50) => "The Minecraft build tools could not be prepared. Check your connection and try Build mod again.",
        Some(60) => "Builder setup is already running in another Bezalel window.",
        _ => "Bezalel could not complete the background builder setup.",
    };
    Err(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_lookup_prefers_bundled_files_and_supports_development_builds() {
        let root = std::env::temp_dir().join(format!("bezalel-resources-{}", uuid::Uuid::new_v4()));
        let packaged = root.join("app");
        let development = root.join("checkout");
        let create = |root: &Path| {
            std::fs::create_dir_all(root.join("builder/templates")).unwrap();
            std::fs::create_dir_all(root.join("builder/fixtures")).unwrap();
            for name in [
                "setup-windows.ps1",
                "prepare.ps1",
                "Containerfile",
                "run.sh",
                "smoke.sh",
            ] {
                std::fs::write(root.join("builder").join(name), b"fixture").unwrap();
            }
        };
        create(&development);
        assert_eq!(
            resolve_resource_root(&packaged, Some(&development)).unwrap(),
            development
        );
        assert!(resolve_resource_root(&packaged, None).is_err());
        create(&packaged);
        assert_eq!(
            resolve_resource_root(&packaged, Some(&development)).unwrap(),
            packaged
        );
        std::fs::remove_file(packaged.join("builder/prepare.ps1")).unwrap();
        assert!(resolve_resource_root(&packaged, None).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn relocated_debug_build_can_find_repository_builder() {
        if cfg!(debug_assertions) {
            let root =
                resource_root(&std::env::temp_dir().join("bezalel-no-bundled-resources")).unwrap();
            assert!(root.join("builder/setup-windows.ps1").is_file());
            assert!(root.join("builder/prepare.ps1").is_file());
        }
    }

    #[cfg(windows)]
    #[test]
    fn powershell_uses_normal_local_drive_paths() {
        let local = Path::new(r"C:\Program Files\Bezalel");
        assert_eq!(
            powershell_path(Path::new(r"\\?\C:\Program Files\Bezalel")),
            local
        );
        assert_eq!(powershell_path(local), local);

        let network = Path::new(r"\\?\UNC\server\share\Bezalel");
        assert_eq!(powershell_path(network), network);
    }

    #[test]
    fn only_known_milestones_are_forwarded() {
        assert_eq!(
            stage_message("BEZALEL_SETUP ready\r\n"),
            Some("The local mod builder is ready.")
        );
        assert_eq!(stage_message("Forge build log"), None);
        assert_eq!(stage_message("BEZALEL_SETUP unknown"), None);
    }

    #[test]
    fn distinguishes_consent_from_failure_after_acceptance() {
        assert_eq!(setup_outcome(Some(10), false), Ok(Outcome::ConsentRequired));
        assert!(setup_outcome(Some(10), true).is_err());
        assert_eq!(setup_outcome(Some(0), false), Ok(Outcome::Ready));
        assert!(setup_outcome(Some(21), true)
            .unwrap_err()
            .contains("Restart Windows"));
    }
}
