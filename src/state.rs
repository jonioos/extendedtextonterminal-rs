use serde::{de::DeserializeOwned, Serialize};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn directory() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("ETOT_STATE_HOME") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("etot"));
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/state/etot"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

pub fn buffer_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = absolute(path)?;
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in absolute.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(directory()?
        .join("buffers")
        .join(format!("{hash:016x}.json")))
}

pub fn absolute(path: &Path) -> io::Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    })
}

pub fn session_path() -> io::Result<PathBuf> {
    Ok(directory()?.join("session.json"))
}

pub fn write_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid state path"))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(temporary.as_file_mut(), value)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn read<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    if fs::metadata(path)?.len() > 64 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state file too large",
        ));
    }
    let file = fs::File::open(path)?;
    serde_json::from_reader(file).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
