//! Git integration through Git's documented CLI. No shell interpolation.
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub status: String,
}
fn run(cwd: &Path, args: &[&str]) -> io::Result<Vec<u8>> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(output.stdout)
}
pub fn root(cwd: &Path) -> io::Result<PathBuf> {
    let bytes = run(cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(String::from_utf8_lossy(&bytes).trim()))
}
pub fn branch(cwd: &Path) -> io::Result<String> {
    let bytes = run(cwd, &["branch", "--show-current"])?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
}
pub fn status(cwd: &Path) -> io::Result<Vec<ChangedFile>> {
    let root = root(cwd)?;
    let bytes = run(
        &root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )?;
    let mut entries = bytes.split(|&b| b == 0).filter(|item| !item.is_empty());
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&entry[..2]).to_string();
        let path = root.join(String::from_utf8_lossy(&entry[3..]).as_ref());
        if status.starts_with('R')
            || status.ends_with('R')
            || status.starts_with('C')
            || status.ends_with('C')
        {
            entries.next();
        }
        files.push(ChangedFile { path, status });
    }
    Ok(files)
}
pub fn diff(cwd: &Path, path: &Path) -> io::Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["diff", "--"])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}
pub fn stage(cwd: &Path, path: &Path) -> io::Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["add", "--"])
        .arg(path)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_diff_and_stage_in_temporary_repo() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]).unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "hello\n").unwrap();
        assert_eq!(status(dir.path()).unwrap().len(), 1);
        stage(dir.path(), &path).unwrap();
        let entries = status(dir.path()).unwrap();
        assert_eq!(entries[0].status, "A ");
        std::fs::write(&path, "changed\n").unwrap();
        assert!(!diff(dir.path(), &path).unwrap().is_empty());
    }
}
