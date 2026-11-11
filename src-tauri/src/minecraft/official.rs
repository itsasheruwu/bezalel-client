use crate::state::{self, err, Result, Target};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub fn version_id(target: Target) -> Result<String> {
    let Target {
        loader,
        minecraft_version,
        loader_version,
        ..
    } = target;
    Ok(match loader {
        "fabric" => format!("fabric-loader-{loader_version}-{minecraft_version}"),
        "forge" => format!("{minecraft_version}-forge-{loader_version}"),
        "neoforge" => format!("neoforge-{loader_version}"),
        _ => return Err("This Minecraft loader is unavailable.".into()),
    })
}

const MANAGED_FOLDERS: [&str; 2] = ["mods", "config/bezalel"];

#[cfg(windows)]
pub fn select_pack(pack: Uuid, name: &str, directory: &Path, version: &str) -> Result<()> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_RoamingAppData, SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };
    // Resolve the real Windows folder even when a development host overrides APPDATA.
    let roaming = unsafe {
        let value = SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, None)
            .map_err(|_| "Could not locate Minecraft Launcher's settings.".to_string())?;
        let path = value.to_string();
        CoTaskMemFree(Some(value.0.cast()));
        PathBuf::from(path.map_err(|_| "Invalid Windows application data path.".to_string())?)
    };
    close_launcher()?;
    clear_stale_launcher_processes()?;
    let root = roaming.join(".minecraft");
    register_pack(&root, pack, name, directory, version)?;
    select_java_page(&root)
}

#[cfg(windows)]
fn close_launcher() -> Result<()> {
    use std::ffi::c_void;
    use windows::{
        core::w,
        Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::WindowsAndMessaging::{
                FindWindowW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
            },
        },
    };
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    let Ok(window) = (unsafe { FindWindowW(None, w!("Minecraft Launcher")) }) else {
        return Ok(());
    };
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut pid));
    }
    let process = unsafe { OpenProcess(0x0010_0000, 0, pid) };
    if process.is_null() {
        return Err("Could not wait for Minecraft Launcher to restart.".into());
    }
    let posted = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    let exited = posted.is_ok() && unsafe { WaitForSingleObject(process, 15000) } == 0;
    unsafe {
        CloseHandle(process);
    }
    if !exited {
        return Err("Minecraft Launcher did not exit. Close it and try Play again.".into());
    }
    Ok(())
}

#[cfg(windows)]
fn clear_stale_launcher_processes() -> Result<()> {
    use windows::core::PWSTR;
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
                PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                PROCESS_TERMINATE,
            },
        },
    };
    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    let snapshot = Owned(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|_| "Could not inspect launcher startup processes.".to_string())?,
    );
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
    while found {
        let name = String::from_utf16_lossy(
            &entry.szExeFile[..entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len())],
        );
        if name.eq_ignore_ascii_case("GameLaunchHelper.exe")
            || name.eq_ignore_ascii_case("Minecraft.exe")
        {
            if let Ok(raw) = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    false,
                    entry.th32ProcessID,
                )
            } {
                let process = Owned(raw);
                let mut buffer = vec![0u16; 32768];
                let mut length = buffer.len() as u32;
                if unsafe {
                    QueryFullProcessImageNameW(
                        process.0,
                        PROCESS_NAME_WIN32,
                        PWSTR(buffer.as_mut_ptr()),
                        &mut length,
                    )
                }
                .is_ok()
                {
                    let path =
                        String::from_utf16_lossy(&buffer[..length as usize]).to_ascii_lowercase();
                    // Other games use this helper name too. Only the official
                    // Minecraft Launcher package is owned by this handoff.
                    if path.contains("\\windowsapps\\microsoft.4297127d64ec6_") {
                        unsafe { TerminateProcess(process.0, 0) }.map_err(|_| {
                            "Could not clear a stuck Minecraft Launcher startup process."
                                .to_string()
                        })?;
                        if unsafe { WaitForSingleObject(process.0, 5000) }.0 != 0 {
                            return Err("Minecraft Launcher is still stopping.".into());
                        }
                    }
                }
            }
        }
        found = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
    }
    Ok(())
}

fn select_java_page(root: &Path) -> Result<()> {
    for name in [
        "launcher_ui_state_microsoft_store.json",
        "launcher_ui_state.json",
    ] {
        let path = root.join(name);
        if !path.exists() {
            continue;
        }
        let original = fs::read_to_string(&path).map_err(err)?;
        let start = original.find('{').ok_or("Invalid launcher UI settings.")?;
        let mut document: serde_json::Value =
            serde_json::from_str(&original[start..]).map_err(err)?;
        let data = document
            .get_mut("data")
            .and_then(|v| v.as_object_mut())
            .ok_or("Invalid launcher UI settings.")?;
        let mut settings: serde_json::Value = serde_json::from_str(
            data.get("UiSettings")
                .and_then(|v| v.as_str())
                .unwrap_or("{}"),
        )
        .map_err(err)?;
        settings
            .as_object_mut()
            .ok_or("Invalid launcher UI settings.")?
            .insert("lastVisitedPage".into(), serde_json::json!("java"));
        data.insert(
            "UiSettings".into(),
            serde_json::json!(serde_json::to_string(&settings).map_err(err)?),
        );
        let backup = path.with_extension("bezalel-backup.json");
        if !backup.exists() {
            fs::copy(&path, backup).map_err(err)?;
        }
        let stage = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
        let updated = format!(
            "{}{}",
            &original[..start],
            serde_json::to_string_pretty(&document).map_err(err)?
        );
        fs::write(&stage, updated).map_err(err)?;
        if fs::read_to_string(&path).map_err(err)? != original {
            let _ = fs::remove_file(stage);
            return Err("Launcher settings changed during pack selection. Try Play again.".into());
        }
        fs::rename(&stage, path).map_err(err)?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn select_pack(_: Uuid, _: &str, _: &Path, _: &str) -> Result<()> {
    Err("Minecraft Launcher integration currently requires Windows.".into())
}

fn register_pack(
    root: &Path,
    pack: Uuid,
    name: &str,
    directory: &Path,
    version: &str,
) -> Result<()> {
    let files: Vec<_> = [
        "launcher_profiles.json",
        "launcher_profiles_microsoft_store.json",
    ]
    .into_iter()
    .map(|file| root.join(file))
    .filter(|file| file.exists())
    .collect();
    if files.is_empty() {
        return Err(
            "Open Minecraft Launcher and sign in once, close it, then try Play again.".into(),
        );
    }
    let id = format!("bezalel-{}", pack.simple());
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    // Validate every profile file before changing any of them.
    let mut updates = Vec::new();
    for file in files {
        let metadata = fs::symlink_metadata(&file).map_err(err)?;
        if !metadata.is_file() || state::is_link(&metadata) {
            return Err("Minecraft Launcher settings must be a regular file.".into());
        }
        let original = fs::read(&file).map_err(err)?;
        let mut document: serde_json::Value = serde_json::from_slice(&original).map_err(|_| {
            "Minecraft Launcher settings could not be read. Close the launcher and try again."
                .to_string()
        })?;
        let object = document
            .as_object_mut()
            .ok_or("Invalid Minecraft Launcher settings.")?;
        let profiles = object
            .entry("profiles")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or("Invalid Minecraft Launcher installations.")?;
        let profile = profiles
            .entry(id.clone())
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or("Invalid Bezalel installation.")?;
        for (key, value) in [
            ("name", serde_json::json!(format!("Bezalel — {name}"))),
            ("type", serde_json::json!("custom")),
            ("gameDir", serde_json::json!(directory)),
            ("lastVersionId", serde_json::json!(version)),
            ("lastUsed", serde_json::json!(now)),
        ] {
            profile.insert(key.into(), value);
        }
        profile
            .entry("created")
            .or_insert_with(|| serde_json::json!(now));
        // Current launchers select the most recently used installation; the old
        // selectedProfile key is ignored and removed when they save profiles.
        object.remove("selectedProfile");
        let updated = serde_json::to_vec_pretty(&document).map_err(err)?;
        updates.push((file, original, updated));
    }
    for (file, original, updated) in updates {
        let backup = file.with_extension("bezalel-backup.json");
        if !backup.exists() {
            fs::copy(&file, &backup).map_err(err)?;
        }
        let stage = file.with_extension(format!("{}.tmp", Uuid::new_v4()));
        let result = (|| {
            fs::write(&stage, updated).map_err(err)?;
            if fs::read(&file).map_err(err)? != original {
                return Err(
                    "Minecraft Launcher settings changed. Close the launcher and try Play again."
                        .into(),
                );
            }
            fs::rename(&stage, &file).map_err(err)
        })();
        if result.is_err() {
            let _ = fs::remove_file(stage);
        }
        result?;
    }
    Ok(())
}

fn ordinary_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.is_dir() || state::is_link(&metadata) {
        return Err("The pack contains an unexpected directory.".into());
    }
    Ok(())
}

fn managed_files(root: &Path) -> Result<BTreeSet<PathBuf>> {
    ordinary_directory(root)?;
    ordinary_directory(&root.join("config"))?;
    let mut files = BTreeSet::new();
    for folder in MANAGED_FOLDERS {
        let directory = root.join(folder);
        ordinary_directory(&directory)?;
        for file in fs::read_dir(directory).map_err(err)? {
            let file = file.map_err(err)?;
            let metadata = fs::symlink_metadata(file.path()).map_err(err)?;
            if !metadata.is_file() || state::is_link(&metadata) {
                return Err("The pack contains an unexpected file.".into());
            }
            files.insert(Path::new(folder).join(file.file_name()));
        }
    }
    Ok(files)
}

fn fingerprint(path: &Path) -> Result<(u64, [u8; 32])> {
    let mut hash = Sha256::new();
    let size = io::copy(&mut fs::File::open(path).map_err(err)?, &mut hash).map_err(err)?;
    Ok((size, hash.finalize().into()))
}

pub fn shared_snapshot(dir: &Path, pack: Uuid, revision: u64) -> Result<PathBuf> {
    let library = dir.parent().ok_or("Missing game library.")?;
    let official = library.join("official-launcher");
    state::ensure_directory(&official)?;
    let parent = official.join(pack.to_string());
    state::ensure_directory(&parent)?;
    let snapshot = parent.join(format!("release-{revision}"));
    let source = managed_files(dir)?;

    match fs::symlink_metadata(&snapshot) {
        Ok(_) => {
            let matches = (|| {
                if source != managed_files(&snapshot)? {
                    return Ok(false);
                }
                for path in &source {
                    if fingerprint(&dir.join(path))? != fingerprint(&snapshot.join(path))? {
                        return Ok(false);
                    }
                }
                Result::Ok(true)
            })();
            if !matches.unwrap_or(false) {
                return Err(format!(
                    "The launcher copy of release {revision} has changed. Close Minecraft, move {} out of the game library, then try Play again.",
                    snapshot.display()
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let stage = parent.join(format!("export-{}", Uuid::new_v4()));
            fs::create_dir(&stage).map_err(err)?;
            let result = (|| {
                for folder in MANAGED_FOLDERS {
                    state::ensure_directory(&stage.join(folder))?;
                }
                for path in &source {
                    fs::copy(dir.join(path), stage.join(path)).map_err(err)?;
                }
                fs::rename(&stage, &snapshot).map_err(err)
            })();
            if result.is_err() {
                let _ = fs::remove_dir_all(stage);
            }
            result?;
        }
        Err(error) => return Err(err(error)),
    }
    Ok(snapshot)
}

#[cfg(windows)]
pub fn open() -> Result<()> {
    use std::os::windows::process::CommandExt;
    let system =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()));
    let output = std::process::Command::new(system.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-Command", "Get-AppxPackage -Name Microsoft.4297127D64EC6 | Select-Object -ExpandProperty InstallLocation"])
        .creation_flags(0x08000000).output().map_err(err)?;
    let helper =
        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()).join("GameLaunchHelper.exe");
    if output.status.success() && helper.is_absolute() && helper.is_file() {
        std::process::Command::new(&helper)
            .current_dir(helper.parent().ok_or("Missing launcher directory.")?)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start Minecraft Launcher: {e}"))?;
        return wait_for_launcher();
    }
    use windows::{
        core::{w, PCWSTR},
        Win32::{
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_LOCAL_SERVER,
                COINIT_APARTMENTTHREADED,
            },
            UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, AO_NONE},
        },
    };

    // Activate the official Store package without passing it a profile or credentials.
    let activated = unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .and_then(|()| {
                let result = (|| {
                    let manager: IApplicationActivationManager =
                        CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?;
                    manager.ActivateApplication(
                        w!("Microsoft.4297127D64EC6_8wekyb3d8bbwe!Minecraft"),
                        PCWSTR::null(),
                        AO_NONE,
                    )
                })();
                CoUninitialize();
                result
            })
    };
    if activated.is_ok() {
        return wait_for_launcher();
    }

    // The standalone launcher uses the standard Program Files install by default.
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(path) = std::env::var_os(variable) {
            let exe = PathBuf::from(path).join(r"Minecraft Launcher\MinecraftLauncher.exe");
            if exe.is_absolute() && exe.is_file() {
                if let Some(path) = exe.to_str() {
                    if crate::studio::platform::open(path).is_ok() {
                        return Ok(());
                    }
                }
            }
        }
    }
    Err("Could not open Minecraft Launcher. Open it from the Windows Start menu or install it from minecraft.net.".into())
}

#[cfg(windows)]
fn wait_for_launcher() -> Result<()> {
    use windows::{core::w, Win32::UI::WindowsAndMessaging::FindWindowW};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if unsafe { FindWindowW(None, w!("Minecraft Launcher")) }.is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err("Minecraft Launcher did not open a window. Try Play again.".into())
}

#[cfg(not(windows))]
pub fn open() -> Result<()> {
    Err("Open Minecraft Launcher from your applications menu.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_selects_pack_and_preserves_other_settings_across_releases() {
        let root = std::env::temp_dir().join(format!("bezalel-profiles-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let original = serde_json::json!({"profiles": {"personal": {"name": "My world"}}, "settings": {"soundOn": false}, "selectedUser": {"account": "unchanged"}});
        for file in [
            "launcher_profiles.json",
            "launcher_profiles_microsoft_store.json",
        ] {
            fs::write(root.join(file), original.to_string()).unwrap();
        }
        let pack = Uuid::new_v4();
        register_pack(&root, pack, "First", &root.join("release-1"), "forge-1").unwrap();
        register_pack(&root, pack, "Renamed", &root.join("release-2"), "forge-2").unwrap();
        for file in [
            "launcher_profiles.json",
            "launcher_profiles_microsoft_store.json",
        ] {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(file)).unwrap()).unwrap();
            let id = format!("bezalel-{}", pack.simple());
            assert!(value.get("selectedProfile").is_none());
            assert!(value["profiles"][&id]["lastUsed"].as_str().is_some());
            assert_eq!(value["profiles"].as_object().unwrap().len(), 2);
            assert_eq!(
                value["profiles"][&id]["gameDir"],
                serde_json::json!(root.join("release-2"))
            );
            assert_eq!(value["profiles"][&id]["lastVersionId"], "forge-2");
            assert_eq!(
                value["profiles"]["personal"],
                original["profiles"]["personal"]
            );
            assert_eq!(value["settings"], original["settings"]);
            assert_eq!(value["selectedUser"], original["selectedUser"]);
        }
        fs::write(root.join("launcher_profiles.json"), b"broken").unwrap();
        assert!(register_pack(&root, pack, "Test", &root, "forge-3").is_err());
        assert_eq!(
            fs::read(root.join("launcher_profiles.json")).unwrap(),
            b"broken"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn java_page_preserves_launcher_header_and_preferences() {
        let root = std::env::temp_dir().join(format!("bezalel-page-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("launcher_ui_state_microsoft_store.json");
        let original = "#$\nlauncher header\n$#\n".to_owned() + &serde_json::json!({"data": {"UiSettings": "{\"lastVisitedPage\":\"bedrock\",\"isMusicEnabled\":false}", "UiEvents": "preserved"}, "formatVersion": 1}).to_string();
        fs::write(&path, &original).unwrap();
        select_java_page(&root).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.starts_with("#$\nlauncher header\n$#\n"));
        let value: serde_json::Value =
            serde_json::from_str(&updated[updated.find('{').unwrap()..]).unwrap();
        let settings: serde_json::Value =
            serde_json::from_str(value["data"]["UiSettings"].as_str().unwrap()).unwrap();
        assert_eq!(settings["lastVisitedPage"], "java");
        assert_eq!(settings["isMusicEnabled"], false);
        assert_eq!(value["data"]["UiEvents"], "preserved");
        assert_eq!(
            fs::read_to_string(path.with_extension("bezalel-backup.json")).unwrap(),
            original
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_launcher_copy_keeps_releases_separate_and_checks_installed_files() {
        let fixture = std::env::temp_dir().join(format!("bezalel-handoff-{}", Uuid::new_v4()));
        let pack = Uuid::new_v4();
        let dir = fixture.join(pack.to_string());
        for folder in ["mods", "config/bezalel", "saves"] {
            fs::create_dir_all(dir.join(folder)).unwrap();
        }
        fs::write(dir.join("mods/grass_sword.jar"), b"release one").unwrap();
        fs::write(dir.join("config/bezalel/grass_sword.json"), b"{}").unwrap();
        fs::write(dir.join("saves/personal-world"), b"saved world").unwrap();

        let first = shared_snapshot(&dir, pack, 1).unwrap();
        assert_eq!(
            fs::read(first.join("mods/grass_sword.jar")).unwrap(),
            b"release one"
        );
        assert_eq!(
            fs::read(first.join("config/bezalel/grass_sword.json")).unwrap(),
            b"{}"
        );
        assert!(!first.join("saves").exists());
        fs::write(first.join("options.txt"), b"personal options").unwrap();
        assert_eq!(shared_snapshot(&dir, pack, 1).unwrap(), first);

        fs::write(dir.join("mods/grass_sword.jar"), b"release two").unwrap();
        let second = shared_snapshot(&dir, pack, 2).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read(first.join("mods/grass_sword.jar")).unwrap(),
            b"release one"
        );
        assert_eq!(
            fs::read(first.join("options.txt")).unwrap(),
            b"personal options"
        );
        assert_eq!(
            fs::read(second.join("mods/grass_sword.jar")).unwrap(),
            b"release two"
        );

        fs::write(second.join("mods/grass_sword.jar"), b"tampered!!!").unwrap();
        assert!(shared_snapshot(&dir, pack, 2)
            .unwrap_err()
            .contains("has changed"));
        assert_eq!(
            fs::read(second.join("mods/grass_sword.jar")).unwrap(),
            b"tampered!!!"
        );
        fs::remove_dir_all(fixture).unwrap();
    }
}
