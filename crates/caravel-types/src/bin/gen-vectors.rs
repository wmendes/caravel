//! Writes the golden vectors to `test-vectors/` (spec §19.2). Run with
//! `cargo gen-vectors`, optionally passing an output directory.

use std::path::PathBuf;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors"));
    std::fs::create_dir_all(&dir).expect("create output directory");
    let mut files: Vec<(&str, String)> = caravel_types::vectors::all()
        .into_iter()
        .map(|f| (f.name, f.contents))
        .collect();
    files.push(("merkle.json", caravel_merkle::vectors::file()));
    for (name, contents) in files {
        let path = dir.join(name);
        std::fs::write(&path, &contents).expect("write vector file");
        println!("wrote {} ({} bytes)", path.display(), contents.len());
    }
}
