//! Embeds the photo skill (`.claude/skills/photo-studio`: `SKILL.md` and every text file under
//! `references/`) into the binary as `$OUT_DIR/skill_files.rs`, the director's and reviewer's
//! default knowledge. Whatever exists is embedded: the skill may be incomplete or missing while it
//! is being written, and files added later trigger a rebuild (Cargo scans watched directories).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Skip files larger than this (the skill is prose; a big file is a mistake).
const MAX_FILE_BYTES: u64 = 2 << 20;
/// Stop after this many files.
const MAX_FILES: usize = 200;

fn main() -> Result<(), String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|e| format!("CARGO_MANIFEST_DIR: {e}"))?;
    let out_dir = std::env::var("OUT_DIR").map_err(|e| format!("OUT_DIR: {e}"))?;
    let skill = Path::new(&manifest).join("../../.claude/skills/photo-studio");
    println!("cargo:rerun-if-changed=build.rs");
    // Watch the nearest existing directory: a missing path would rerun this script on every build.
    let mut watch = skill.clone();
    while !watch.exists() {
        match watch.parent() {
            Some(p) => watch = p.to_path_buf(),
            None => break,
        }
    }
    println!("cargo:rerun-if-changed={}", watch.display());

    let mut files: Vec<(String, String)> = Vec::new();
    if let Some(text) = read_text(&skill.join("SKILL.md")) {
        files.push(("SKILL.md".to_string(), text));
    }
    let mut refs = Vec::new();
    collect(&skill.join("references"), &mut refs, 0);
    refs.sort();
    for path in refs {
        if files.len() >= MAX_FILES {
            break;
        }
        let Ok(rel) = path.strip_prefix(&skill) else { continue };
        let rel = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        if let Some(text) = read_text(&path) {
            files.push((rel, text));
        }
    }

    let mut out = String::from("/// The photo skill compiled in by `build.rs`: (path relative to the skill, contents).\n");
    out.push_str("pub static EMBEDDED: &[(&str, &str)] = &[\n");
    for (name, text) in &files {
        // `{:?}` of a `str` is a valid Rust string literal.
        writeln!(out, "    ({name:?}, {text:?}),").map_err(|e| e.to_string())?;
    }
    out.push_str("];\n");
    let dest = Path::new(&out_dir).join("skill_files.rs");
    std::fs::write(&dest, out).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Regular files under `dir`, recursively (hidden entries skipped).
fn collect(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect(&path, out, depth + 1),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// The file's text if it is a reasonably small UTF-8 file.
fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}
