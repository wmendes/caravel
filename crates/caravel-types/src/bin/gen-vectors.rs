//! Writes the golden vectors to `test-vectors/` (spec §19.2). Run with
//! `cargo gen-vectors`, optionally passing an output directory.

use std::path::PathBuf;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors"));
    std::fs::create_dir_all(&dir).expect("create output directory");
    for f in caravel_types::vectors::all() {
        let path = dir.join(f.name);
        std::fs::write(&path, &f.contents).expect("write vector file");
        println!("wrote {} ({} bytes)", path.display(), f.contents.len());
    }
}
