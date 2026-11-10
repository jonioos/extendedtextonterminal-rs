//! Minimal LSP stdio client: initialize, full document synchronization,
//! and published diagnostics. More language requests are added separately.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub row: usize,
    pub col: usize,
    pub severity: u64,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub new_text: String,
}
#[derive(Debug, Clone)]
pub struct CodeAction {
    pub title: String,
    pub edits: Vec<TextEdit>,
}
impl TextEdit {
    fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            start: (
                value.pointer("/range/start/line")?.as_u64()? as usize,
                value.pointer("/range/start/character")?.as_u64()? as usize,
            ),
            end: (
                value.pointer("/range/end/line")?.as_u64()? as usize,
                value.pointer("/range/end/character")?.as_u64()? as usize,
            ),
            new_text: value.get("newText")?.as_str()?.to_owned(),
        })
    }
}
pub struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: u64,
    pending: HashMap<u64, Value>,
    pub diagnostics: Vec<Diagnostic>,
    raw_diagnostics: Vec<Value>,
    uri: String,
    version: u64,
}
fn write_message(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}
fn read_message(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if header == "\r\n" {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.filter(|&n| n <= 16 * 1024 * 1024).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing or excessive LSP Content-Length",
        )
    })?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
fn file_uri(path: &Path) -> io::Result<String> {
    let canonical = path.canonicalize()?;
    let mut uri = String::from("file://");
    for byte in canonical.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char)
        } else {
            uri.push_str(&format!("%{byte:02X}"))
        }
    }
    Ok(uri)
}
impl Client {
    pub fn start(
        command: &[String],
        root: &Path,
        path: &Path,
        language: &str,
        text: &str,
    ) -> io::Result<Self> {
        let Some(program) = command.first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty LSP command",
            ));
        };
        let mut child = Command::new(program)
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("LSP stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("LSP stdout unavailable"))?;
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(message) = read_message(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let uri = file_uri(path)?;
        let mut client = Self {
            child,
            stdin,
            messages,
            next_id: 1,
            pending: HashMap::new(),
            diagnostics: Vec::new(),
            raw_diagnostics: Vec::new(),
            uri,
            version: 1,
        };
        let root_uri = file_uri(root)?;
        let id = client.request(
            "initialize",
            json!({
                "processId": std::process::id(), "rootUri": root_uri,
                "capabilities": { "general": { "positionEncodings": ["utf-16"] },
                    "textDocument": { "synchronization": { "didSave": true },
                        "publishDiagnostics": {}, "definition": {}, "formatting": {},
                        "documentSymbol": {}, "codeAction": {} } },
                "clientInfo": { "name": "ETOT", "version": "0.1.0" }
            }),
        )?;
        client.wait_response(id, Duration::from_secs(5))?;
        client.notify("initialized", json!({}))?;
        client.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": client.uri, "languageId": language, "version": client.version, "text": text
            }}),
        )?;
        Ok(client)
    }
    fn request(&mut self, method: &str, params: Value) -> io::Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        write_message(
            &mut self.stdin,
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )?;
        Ok(id)
    }
    fn notify(&mut self, method: &str, params: Value) -> io::Result<()> {
        write_message(
            &mut self.stdin,
            &json!({"jsonrpc":"2.0", "method":method, "params":params}),
        )
    }
    fn process(&mut self, message: Value) {
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            self.pending.insert(id, message);
            return;
        }
        if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && message.pointer("/params/uri").and_then(Value::as_str) == Some(&self.uri)
        {
            if message
                .pointer("/params/version")
                .and_then(Value::as_u64)
                .is_some_and(|version| version < self.version)
            {
                return;
            }
            self.raw_diagnostics = message
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            self.diagnostics = message
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|d| {
                            Some(Diagnostic {
                                row: d.pointer("/range/start/line")?.as_u64()? as usize,
                                col: d.pointer("/range/start/character")?.as_u64()? as usize,
                                severity: d.get("severity").and_then(Value::as_u64).unwrap_or(3),
                                message: d.get("message")?.as_str()?.to_owned(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
    }
    fn wait_response(&mut self, id: u64, timeout: Duration) -> io::Result<Value> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(response) = self.pending.remove(&id) {
                if let Some(error) = response.get("error") {
                    return Err(io::Error::other(error.to_string()));
                }
                return Ok(response.get("result").cloned().unwrap_or(Value::Null));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let message = self
                .messages
                .recv_timeout(remaining)
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LSP response timeout"))?;
            self.process(message);
        }
    }
    pub fn poll(&mut self) -> bool {
        let before = self.diagnostics.clone();
        while let Ok(message) = self.messages.try_recv() {
            self.process(message)
        }
        self.diagnostics != before
    }
    pub fn did_change(&mut self, text: &str) -> io::Result<()> {
        self.version += 1;
        self.diagnostics.clear();
        self.raw_diagnostics.clear();
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri":self.uri, "version":self.version},
                "contentChanges": [{"text":text}]
            }),
        )
    }
    pub fn completion(&mut self, row: usize, col_utf16: usize) -> io::Result<Vec<String>> {
        let id = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri":self.uri}, "position": {"line":row, "character":col_utf16},
                "context": {"triggerKind":1}
            }),
        )?;
        let result = self.wait_response(id, Duration::from_secs(5))?;
        let items = result
            .as_array()
            .or_else(|| result.get("items").and_then(Value::as_array));
        Ok(items
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        if item.get("insertTextFormat").and_then(Value::as_u64) == Some(2) {
                            return None;
                        }
                        item.get("insertText")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("label").and_then(Value::as_str))
                            .map(str::to_owned)
                    })
                    .filter(|text| !text.contains('\n'))
                    .take(100)
                    .collect()
            })
            .unwrap_or_default())
    }
    pub fn formatting(&mut self) -> io::Result<Vec<TextEdit>> {
        let id = self.request(
            "textDocument/formatting",
            json!({
                "textDocument": {"uri":self.uri},
                "options": {"tabSize":4, "insertSpaces":true}
            }),
        )?;
        let result = self.wait_response(id, Duration::from_secs(8))?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let items = result.as_array().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP formatting response is not an array",
            )
        })?;
        items
            .iter()
            .map(|item| {
                TextEdit::parse(item).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP text edit")
                })
            })
            .collect()
    }
    pub fn symbols(&mut self) -> io::Result<Vec<(String, usize, usize)>> {
        let id = self.request(
            "textDocument/documentSymbol",
            json!({
                "textDocument": {"uri":self.uri}
            }),
        )?;
        let result = self.wait_response(id, Duration::from_secs(5))?;
        let mut symbols = Vec::new();
        if let Some(items) = result.as_array() {
            for item in items {
                collect_symbols(item, &mut symbols);
            }
        }
        Ok(symbols)
    }
    pub fn code_actions(&mut self, row: usize, col_utf16: usize) -> io::Result<Vec<CodeAction>> {
        let relevant: Vec<Value> = self
            .raw_diagnostics
            .iter()
            .filter(|item| {
                item.pointer("/range/start/line").and_then(Value::as_u64) == Some(row as u64)
            })
            .cloned()
            .collect();
        let range = relevant
            .first()
            .and_then(|item| item.get("range"))
            .cloned()
            .unwrap_or_else(|| {
                json!({"start":{"line":row,"character":col_utf16},
                   "end":{"line":row,"character":col_utf16}})
            });
        let id = self.request(
            "textDocument/codeAction",
            json!({
                "textDocument":{"uri":self.uri}, "range":range,
                "context":{"diagnostics":relevant}
            }),
        )?;
        let result = self.wait_response(id, Duration::from_secs(5))?;
        Ok(result
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| parse_code_action(item, &self.uri))
                    .take(100)
                    .collect()
            })
            .unwrap_or_default())
    }
    pub fn rename(
        &mut self,
        row: usize,
        col_utf16: usize,
        new_name: &str,
    ) -> io::Result<Vec<(PathBuf, Vec<TextEdit>)>> {
        let id = self.request(
            "textDocument/rename",
            json!({
                "textDocument":{"uri":self.uri},
                "position":{"line":row,"character":col_utf16},
                "newName":new_name
            }),
        )?;
        let result = self.wait_response(id, Duration::from_secs(8))?;
        parse_workspace_edit(&result)
    }
    pub fn hover(&mut self, row: usize, col_utf16: usize) -> io::Result<Option<String>> {
        let id = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri":self.uri}, "position": {"line":row, "character":col_utf16}
            }),
        )?;
        let result = self.wait_response(id, Duration::from_secs(5))?;
        let contents = &result["contents"];
        let text = if let Some(value) = contents.as_str() {
            value.to_owned()
        } else if let Some(value) = contents.get("value").and_then(Value::as_str) {
            value.to_owned()
        } else if let Some(items) = contents.as_array() {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .or_else(|| item.get("value").and_then(Value::as_str))
                })
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::new()
        };
        Ok((!text.is_empty()).then_some(
            text.lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .to_owned(),
        ))
    }
    pub fn definition(
        &mut self,
        row: usize,
        col_utf16: usize,
    ) -> io::Result<Option<(PathBuf, usize, usize)>> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let id = self.request("textDocument/definition", json!({
                "textDocument": {"uri":self.uri}, "position": {"line":row, "character":col_utf16}
            }))?;
            let result = self.wait_response(id, Duration::from_secs(5))?;
            let location = result
                .as_array()
                .and_then(|items| items.first())
                .unwrap_or(&result);
            if let Some(found) = parse_location(location) {
                return Ok(Some(found));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    pub fn references(
        &mut self,
        row: usize,
        col_utf16: usize,
    ) -> io::Result<Vec<(PathBuf, usize, usize)>> {
        let id = self.request(
            "textDocument/references",
            json!({
                "textDocument": {"uri":self.uri}, "position": {"line":row, "character":col_utf16},
                "context": {"includeDeclaration":true}
            }),
        )?;
        let result = self.wait_response(id, Duration::from_secs(5))?;
        Ok(result
            .as_array()
            .map(|items| items.iter().filter_map(parse_location).collect())
            .unwrap_or_default())
    }
}
fn parse_location(location: &Value) -> Option<(PathBuf, usize, usize)> {
    let uri = location
        .get("uri")
        .or_else(|| location.get("targetUri"))?
        .as_str()?;
    let position = location
        .pointer("/range/start")
        .or_else(|| location.pointer("/targetSelectionRange/start"))
        .or_else(|| location.pointer("/targetRange/start"))?;
    Some((
        decode_file_uri(uri)?,
        position.get("line")?.as_u64()? as usize,
        position.get("character")?.as_u64()? as usize,
    ))
}
fn collect_symbols(value: &Value, out: &mut Vec<(String, usize, usize)>) {
    if let (Some(name), Some(position)) = (
        value.get("name").and_then(Value::as_str),
        value
            .pointer("/selectionRange/start")
            .or_else(|| value.pointer("/location/range/start")),
    ) {
        if let (Some(row), Some(col)) = (
            position.get("line").and_then(Value::as_u64),
            position.get("character").and_then(Value::as_u64),
        ) {
            out.push((name.to_owned(), row as usize, col as usize));
        }
    }
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            collect_symbols(child, out);
        }
    }
}
fn parse_code_action(value: &Value, uri: &str) -> Option<CodeAction> {
    let title = value.get("title")?.as_str()?.to_owned();
    let edit = value.get("edit")?;
    let mut edits = Vec::new();
    if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (target, items) in changes {
            if target != uri {
                return None;
            }
            for item in items.as_array()? {
                edits.push(TextEdit::parse(item)?);
            }
        }
    } else {
        let changes = edit.get("documentChanges").and_then(Value::as_array)?;
        for change in changes {
            if change.pointer("/textDocument/uri")?.as_str()? != uri {
                return None;
            }
            for item in change.get("edits")?.as_array()? {
                edits.push(TextEdit::parse(item)?);
            }
        }
    }
    (!edits.is_empty()).then_some(CodeAction { title, edits })
}
fn parse_workspace_edit(value: &Value) -> io::Result<Vec<(PathBuf, Vec<TextEdit>)>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let mut grouped: std::collections::BTreeMap<PathBuf, Vec<TextEdit>> =
        std::collections::BTreeMap::new();
    if let Some(changes) = value.get("changes").and_then(Value::as_object) {
        for (uri, items) in changes {
            let path = decode_file_uri(uri).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Rename returned unsupported URI",
                )
            })?;
            for item in items.as_array().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Rename changes are not an array",
                )
            })? {
                grouped
                    .entry(path.clone())
                    .or_default()
                    .push(TextEdit::parse(item).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Rename returned invalid text edit",
                        )
                    })?);
            }
        }
    } else if let Some(changes) = value.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            let uri = change
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Rename returned unsupported file operation",
                    )
                })?;
            let path = decode_file_uri(uri).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Rename returned unsupported URI",
                )
            })?;
            for item in change
                .get("edits")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Rename returned invalid document edits",
                    )
                })?
            {
                grouped
                    .entry(path.clone())
                    .or_default()
                    .push(TextEdit::parse(item).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Rename returned invalid text edit",
                        )
                    })?);
            }
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Rename returned unsupported workspace edit",
        ));
    }
    Ok(grouped.into_iter().collect())
}
fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < encoded.len() {
        if encoded[i] == b'%' && i + 2 < encoded.len() {
            let hex = std::str::from_utf8(&encoded[i + 1..i + 3]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(encoded[i]);
            i += 1
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.notify("exit", json!({}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framed_json_roundtrip_and_uri() {
        let value = json!({"jsonrpc":"2.0","method":"test","params":{"text":"é"}});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &value).unwrap();
        assert_eq!(
            read_message(&mut BufReader::new(bytes.as_slice())).unwrap(),
            value
        );
        assert_eq!(
            decode_file_uri("file:///tmp/a%20b.rs"),
            Some(PathBuf::from("/tmp/a b.rs"))
        );
    }
    #[test]
    fn code_action_accepts_only_local_text_edits() {
        let action = json!({"title":"Fix","edit":{"changes":{
            "file:///tmp/main.rs":[{"range":{"start":{"line":0,"character":1},
                "end":{"line":0,"character":3}},"newText":"x"}]
        }}});
        let parsed = parse_code_action(&action, "file:///tmp/main.rs").unwrap();
        assert_eq!(parsed.title, "Fix");
        assert_eq!(parsed.edits[0].start, (0, 1));
        assert!(parse_code_action(&action, "file:///tmp/other.rs").is_none());
    }
    #[test]
    fn workspace_edit_parses_multiple_files_and_rejects_file_ops() {
        let edit = json!({"changes":{
            "file:///tmp/a.rs":[{"range":{"start":{"line":0,"character":0},
                "end":{"line":0,"character":3}},"newText":"new"}],
            "file:///tmp/b.rs":[{"range":{"start":{"line":1,"character":2},
                "end":{"line":1,"character":5}},"newText":"new"}]
        }});
        let files = parse_workspace_edit(&edit).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, PathBuf::from("/tmp/a.rs"));
        assert!(
            parse_workspace_edit(&json!({"documentChanges":[{"kind":"rename",
            "oldUri":"file:///tmp/a.rs", "newUri":"file:///tmp/b.rs"}]}))
            .is_err()
        );
    }
}
