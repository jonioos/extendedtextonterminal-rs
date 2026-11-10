use etot_rs::{Editor, Key};
use std::io::Write;
use std::time::Instant;

fn main() {
    let mut file = tempfile::NamedTempFile::new().expect("create benchmark file");
    for i in 0..100_000 {
        writeln!(file, "line {i:06} with text for a medium-sized source file").expect("write line");
    }
    let size = file.as_file().metadata().expect("metadata").len();
    let start = Instant::now();
    let mut editor = Editor::open(Some(file.path().to_path_buf()), false).expect("open file");
    let open = start.elapsed();
    editor.handle(Key::Char('/'));
    for c in "line 099999".chars() {
        editor.handle(Key::Char(c))
    }
    let start = Instant::now();
    editor.handle(Key::Enter);
    let search = start.elapsed();
    assert_eq!(editor.row, 99_999);
    let start = Instant::now();
    editor.handle(Key::Char('x'));
    let edit = start.elapsed();
    editor.handle(Key::Char('u'));
    let start = Instant::now();
    editor.handle(Key::Char('/'));
    for c in "line".chars() {
        editor.handle(Key::Char(c));
    }
    editor.handle(Key::Enter);
    let common_search = start.elapsed();
    println!("{size} bytes, 100000 lines: open {open:?}, rare search {search:?}, edit {edit:?}, common incremental search {common_search:?}");
}
