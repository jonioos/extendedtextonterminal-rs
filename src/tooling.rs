use crate::config::ToolCommand;
use regex::Regex;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct ToolOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(spec: &ToolCommand, file: &Path, cwd: &Path, input: &str) -> io::Result<ToolOutput> {
    if input.len() > 8 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file exceeds tool input limit",
        ));
    }
    let (program, arguments) = spec
        .command
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty tool command"))?;
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let executable = if matches!(program.as_str(), "cargo" | "rustfmt") {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".cargo/bin").join(program))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from(program))
    } else {
        PathBuf::from(program)
    };
    let working_dir = if program == "cargo" {
        file.parent()
            .and_then(|parent| {
                parent
                    .ancestors()
                    .find(|dir| dir.join("Cargo.toml").is_file())
            })
            .unwrap_or(cwd)
    } else {
        cwd
    };
    let mut child = Command::new(executable)
        .args(
            arguments
                .iter()
                .map(|arg| arg.replace("{file}", &file.to_string_lossy())),
        )
        .current_dir(working_dir)
        .stdin(if spec.stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let text = input.to_owned();
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tool exceeded 15 seconds",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    fn collect(file: &mut File) -> io::Result<String> {
        if file.metadata()?.len() > 8 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "tool output too large",
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(text)
    }
    Ok(ToolOutput {
        success: status.success(),
        stdout: collect(&mut stdout)?,
        stderr: collect(&mut stderr)?,
    })
}

pub fn diagnostics(
    output: &ToolOutput,
    file: &Path,
    cwd: &Path,
) -> Vec<(PathBuf, usize, usize, String)> {
    let pattern = Regex::new(r"^(.+?):(\d+):(\d+):\s*(.+)$").unwrap();
    output
        .stdout
        .lines()
        .chain(output.stderr.lines())
        .filter_map(|line| {
            let captures = pattern.captures(line)?;
            let name = Path::new(captures.get(1)?.as_str());
            let path = if name == Path::new("-") || name == Path::new("stdin") {
                file.to_path_buf()
            } else if name.is_absolute() {
                name.to_path_buf()
            } else {
                cwd.join(name)
            };
            let row = captures
                .get(2)?
                .as_str()
                .parse::<usize>()
                .ok()?
                .saturating_sub(1);
            let col = captures
                .get(3)?
                .as_str()
                .parse::<usize>()
                .ok()?
                .saturating_sub(1);
            Some((path, row, col, captures.get(4)?.as_str().to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn format_uses_stdin_and_lint_locations_are_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let spec = ToolCommand {
            command: vec!["cat".into()],
            stdin: true,
        };
        let output = run(&spec, &dir.path().join("a.py"), dir.path(), "hello\n").unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, "hello\n");
        let reported = ToolOutput {
            success: false,
            stdout: "a.py:2:3: E1 problem\n".into(),
            stderr: String::new(),
        };
        assert_eq!(
            diagnostics(&reported, &dir.path().join("a.py"), dir.path())[0].1,
            1
        );
    }
}
