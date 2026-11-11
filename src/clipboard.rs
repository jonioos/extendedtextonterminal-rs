use std::io;

pub fn copy(text: &str) -> io::Result<()> {
    let mut clipboard = arboard::Clipboard::new().map_err(io::Error::other)?;
    clipboard.set_text(text).map_err(io::Error::other)
}

pub fn paste() -> io::Result<String> {
    let mut clipboard = arboard::Clipboard::new().map_err(io::Error::other)?;
    clipboard.get_text().map_err(io::Error::other)
}
