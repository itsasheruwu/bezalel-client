use crate::{
    state::{err, Result},
    studio::{platform, write_atomic},
};
use std::{fs, io, path::Path};

pub fn read(root: &Path, name: &str) -> Result<Option<String>> {
    match fs::read(root.join(name)) {
        Ok(bytes) => {
            let plaintext = platform::protect(&bytes, false)?;
            String::from_utf8(plaintext).map(Some).map_err(err)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(err(error)),
    }
}

pub fn write(root: &Path, name: &str, value: &str) -> Result<()> {
    let encrypted = platform::protect(value.as_bytes(), true)?;
    write_atomic(&root.join(name), &encrypted)
}

pub fn remove(root: &Path, name: &str) -> Result<()> {
    match fs::remove_file(root.join(name)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(err(error)),
    }
}
