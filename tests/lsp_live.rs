use etot_rs::lsp::Client;
use std::fs;
use std::process::Command;

#[test]
fn rust_analyzer_initialize_and_definition() {
    if Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"lsp_smoke\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    let source = "fn target() {}\nfn main() { target(); }\n";
    let path = dir.path().join("src/main.rs");
    fs::write(&path, source).unwrap();
    let mut client =
        Client::start(&["rust-analyzer".into()], dir.path(), &path, "rust", source).unwrap();
    let location = client.definition(1, 12).unwrap();
    assert_eq!(location.map(|(_, row, _)| row), Some(0));
    let references = client.references(0, 4).unwrap();
    assert!(references.iter().any(|(_, row, _)| *row == 1));
    let symbols = client.symbols().unwrap();
    assert!(symbols.iter().any(|(name, _, _)| name == "target"));
    let rename = client.rename(1, 12, "renamed").unwrap();
    assert!(rename
        .iter()
        .flat_map(|(_, edits)| edits)
        .any(|edit| edit.new_text == "renamed"));
    client
        .did_change("fn target() {}\nfn main() { target(); }\n")
        .unwrap();
    client.formatting().unwrap();
}

#[test]
fn rust_analyzer_publishes_diagnostics() {
    if Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"lsp_diag\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    let source = "fn main() { let x = ; }\n";
    let path = dir.path().join("src/main.rs");
    fs::write(&path, source).unwrap();
    let mut client =
        Client::start(&["rust-analyzer".into()], dir.path(), &path, "rust", source).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while client.diagnostics.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        client.poll();
    }
    assert!(
        !client.diagnostics.is_empty(),
        "rust-analyzer sent no diagnostics"
    );
    client.code_actions(0, 20).unwrap();
}
