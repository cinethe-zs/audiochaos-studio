//! Compiles every tracks/<name>/compose.rs into the studio, so that a new
//! track needs nothing but its file.

use std::fmt::Write;
use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let tracks = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("tracks");
    println!("cargo:rerun-if-changed={}", tracks.display());
    let mut names: Vec<String> = fs::read_dir(&tracks)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("compose.rs").is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut code = String::new();
    for name in &names {
        let path = tracks.join(name).join("compose.rs");
        writeln!(
            code,
            "#[path = {:?}]\nmod {};",
            path.to_string_lossy(),
            module(name)
        )
        .unwrap();
    }
    code.push_str("/// Every track under tracks/, by folder name.\n");
    code.push_str("pub const TRACKS: &[(&str, Compose)] = &[\n");
    for name in &names {
        writeln!(code, "    ({name:?}, {}::compose),", module(name)).unwrap();
    }
    code.push_str("];\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("tracks.rs"),
        code,
    )
    .unwrap();
}

/// A Rust module name for a track folder.
fn module(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("track_{safe}")
}
