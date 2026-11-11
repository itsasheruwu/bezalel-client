use crate::studio::write_atomic;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MARKER: &str = "game-process.json";
const UNKNOWN: &str = "Bezalel could not verify whether Minecraft is running for this pack.";

#[derive(Serialize, Deserialize)]
struct Marker {
    pid: u32,
    creation_time: u64,
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

fn directory_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() && ordinary(&info) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        _ => Err(UNKNOWN.into()),
    }
}

fn read_marker(path: &Path) -> Result<Option<Marker>, String> {
    match fs::symlink_metadata(path) {
        Ok(info) if info.is_file() && ordinary(&info) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        _ => return Err(UNKNOWN.into()),
    }
    let mut open = fs::OpenOptions::new();
    open.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = open.open(path).map_err(|_| UNKNOWN)?;
    let info = file.metadata().map_err(|_| UNKNOWN)?;
    if !info.is_file() || !ordinary(&info) || info.len() > 512 {
        return Err(UNKNOWN.into());
    }
    let mut bytes = Vec::new();
    file.take(513)
        .read_to_end(&mut bytes)
        .map_err(|_| UNKNOWN)?;
    if bytes.len() > 512 {
        return Err(UNKNOWN.into());
    }
    let marker: Marker = serde_json::from_slice(&bytes).map_err(|_| UNKNOWN)?;
    if marker.pid == 0 || marker.creation_time == 0 {
        return Err(UNKNOWN.into());
    }
    Ok(Some(marker))
}

fn process_dir(game_dir: &Path) -> PathBuf {
    game_dir.join(".bezalel")
}

/// The caller holds the pack lock during this check and any ensuing changes.
pub fn ensure_idle(game_dir: &Path) -> Result<(), String> {
    if !directory_exists(game_dir)? {
        return Ok(());
    }
    let directory = process_dir(game_dir);
    if !directory_exists(&directory)? {
        return Ok(());
    }
    let path = directory.join(MARKER);
    let Some(marker) = read_marker(&path)? else {
        return Ok(());
    };
    if active_creation_time(marker.pid)? == Some(marker.creation_time) {
        return Err(
            "Close Minecraft for this pack before playing again or applying updates.".into(),
        );
    }
    fs::remove_file(path)
        .map_err(|_| "Could not clear the previous Minecraft process marker.".into())
}

/// The caller holds the pack lock and stops the spawned child if recording fails.
pub fn record(game_dir: &Path, pid: u32) -> Result<(), String> {
    ensure_idle(game_dir)?;
    let creation_time = active_creation_time(pid)?
        .ok_or("Minecraft exited before Bezalel could record the game process.")?;
    if !directory_exists(game_dir)? {
        return Err("The Minecraft game directory is missing.".into());
    }
    let directory = process_dir(game_dir);
    if !directory_exists(&directory)? {
        fs::create_dir(&directory)
            .map_err(|_| "Could not create the Minecraft process directory.")?;
    }
    let marker = serde_json::to_vec(&Marker { pid, creation_time })
        .map_err(|_| "Could not encode the Minecraft process marker.")?;
    write_atomic(&directory.join(MARKER), &marker)
}

#[cfg(not(windows))]
fn active_creation_time(_pid: u32) -> Result<Option<u64>, String> {
    Err(UNKNOWN.into())
}

#[cfg(windows)]
fn active_creation_time(pid: u32) -> Result<Option<u64>, String> {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(process: *mut c_void) -> i32;
        fn WaitForSingleObject(process: *mut c_void, millis: u32) -> u32;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    struct Process(*mut c_void);
    impl Drop for Process {
        fn drop(&mut self) {
            // The wrapper owns a non-null handle returned by OpenProcess.
            unsafe { CloseHandle(self.0) };
        }
    }

    const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x102;

    if pid == 0 {
        return Err(UNKNOWN.into());
    }
    // The non-inheritable handle identifies one process even if its PID later changes hands.
    let raw = unsafe { OpenProcess(QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        return match io::Error::last_os_error().raw_os_error() {
            Some(ERROR_INVALID_PARAMETER) => Ok(None),
            _ => Err(UNKNOWN.into()),
        };
    }
    let process = Process(raw);
    // A zero timeout asks whether the process is already signaled without blocking.
    match unsafe { WaitForSingleObject(process.0, 0) } {
        WAIT_OBJECT_0 => return Ok(None),
        WAIT_TIMEOUT => {}
        _ => return Err(UNKNOWN.into()),
    }
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // Each FILETIME points to separate writable storage that lives through the call.
    if unsafe { GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user) } == 0
    {
        return Err(UNKNOWN.into());
    }
    let timestamp = u64::from(creation.high) << 32 | u64::from(creation.low);
    if timestamp == 0 {
        return Err(UNKNOWN.into());
    }
    Ok(Some(timestamp))
}
