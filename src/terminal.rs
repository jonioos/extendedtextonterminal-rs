use crate::Key;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{self, Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub struct TerminalPane {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
}

impl TerminalPane {
    pub fn start(rows: u16, cols: u16, cwd: &Path) -> io::Result<Self> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)?;
        let mut command =
            CommandBuilder::new(std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into()));
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let (sender, output) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            while let Ok(count) = reader.read(&mut chunk) {
                if count == 0 || sender.send(chunk[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            master: pair.master,
            child,
            writer,
            output,
            parser: vt100::Parser::new(
                NonZeroU16::new(rows).unwrap(),
                NonZeroU16::new(cols).unwrap(),
                256,
            ),
            rows,
            cols,
        })
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..64 {
            match self.output.try_recv() {
                Ok(chunk) => {
                    self.parser.process(&chunk);
                    changed = true;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        changed
    }
    pub fn resize(&mut self, rows: u16, cols: u16) -> io::Result<()> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if (rows, cols) != (self.rows, self.cols) {
            self.master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(io::Error::other)?;
            self.parser.set_size(
                NonZeroU16::new(rows).unwrap(),
                NonZeroU16::new(cols).unwrap(),
            );
            (self.rows, self.cols) = (rows, cols);
        }
        Ok(())
    }
    pub fn send(&mut self, key: Key) -> io::Result<()> {
        let bytes: Vec<u8> = match key {
            Key::Char(ch) => ch.to_string().into_bytes(),
            Key::Enter => vec![b'\r'],
            Key::Esc => vec![27],
            Key::Backspace => vec![127],
            Key::Delete => b"\x1b[3~".to_vec(),
            Key::Up => b"\x1b[A".to_vec(),
            Key::Down => b"\x1b[B".to_vec(),
            Key::Right => b"\x1b[C".to_vec(),
            Key::Left => b"\x1b[D".to_vec(),
            Key::Home => b"\x1b[H".to_vec(),
            Key::End => b"\x1b[F".to_vec(),
            Key::PageUp => b"\x1b[5~".to_vec(),
            Key::PageDown => b"\x1b[6~".to_vec(),
            Key::Ctrl(' ') => vec![0],
            Key::Ctrl(ch) if ch.is_ascii_lowercase() => vec![ch as u8 - b'a' + 1],
            Key::Ctrl(_) => Vec::new(),
        };
        self.writer.write_all(&bytes)?;
        self.writer.flush()
    }
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
    pub fn exited(&mut self) -> io::Result<bool> {
        Ok(self.child.try_wait().map_err(io::Error::other)?.is_some())
    }
}

impl Drop for TerminalPane {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
