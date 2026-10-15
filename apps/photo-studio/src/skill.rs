//! The photo skill: the director's and reviewer's knowledge (`SKILL.md` + `references/*`).
//!
//! The copy compiled into the binary (see `build.rs`) is the default; `--skill DIR` reads a
//! directory with the same layout at startup instead. The skill's routing table also gives each
//! model its categories for `GET /api/models`.

use std::collections::HashMap;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/skill_files.rs"));

/// Limits for a skill read at runtime.
const MAX_FILE_BYTES: u64 = 2 << 20;
const MAX_TOTAL_BYTES: usize = 8 << 20;
const MAX_FILES: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    /// Where it came from, for logs: "embedded" or the directory.
    pub source: String,
    /// (path relative to the skill directory, text), `SKILL.md` first.
    pub files: Vec<(String, String)>,
}

impl Skill {
    pub fn embedded() -> Skill {
        Skill { source: "embedded".into(), files: EMBEDDED.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect() }
    }

    /// Read `SKILL.md` and every text file under `references/` from `dir`.
    pub fn from_dir(dir: &Path) -> Result<Skill, String> {
        if !dir.is_dir() {
            return Err(format!("skill directory {}: not a directory", dir.display()));
        }
        let mut files = Vec::new();
        let mut total = 0usize;
        let mut add = |rel: String, path: &Path, files: &mut Vec<(String, String)>| -> Result<(), String> {
            let Some(bytes) = crate::store::read_capped(path, MAX_FILE_BYTES)? else { return Ok(()) };
            let Ok(text) = String::from_utf8(bytes) else { return Ok(()) }; // not text: skip
            total += text.len();
            if total > MAX_TOTAL_BYTES || files.len() >= MAX_FILES {
                return Err(format!("skill directory {}: too large", dir.display()));
            }
            files.push((rel, text));
            Ok(())
        };
        add("SKILL.md".into(), &dir.join("SKILL.md"), &mut files)?;
        let mut refs = Vec::new();
        collect(&dir.join("references"), &mut refs, 0);
        refs.sort();
        for path in refs {
            let Ok(rel) = path.strip_prefix(dir) else { continue };
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
            add(rel, &path, &mut files)?;
        }
        if files.is_empty() {
            return Err(format!("skill directory {}: no SKILL.md and no references/", dir.display()));
        }
        Ok(Skill { source: dir.display().to_string(), files })
    }

    /// The skill as one text block for a system prompt.
    pub fn prompt_text(&self) -> String {
        let mut out = String::new();
        for (name, text) in &self.files {
            out.push_str(&format!("<skill_file path=\"{name}\">\n{}\n</skill_file>\n\n", text.trim_end()));
        }
        out
    }

    /// Model id → routing categories, from the skill's routing table(s): markdown tables in a
    /// file whose name mentions "routing", under a heading that mentions "routing", or with a
    /// header cell that mentions "category". The category column is the one whose header
    /// mentions "category", else the first; it may name several categories separated by `/` or
    /// `,`. Every model id written in backticks in the other columns belongs to them.
    pub fn routing(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for (name, text) in &self.files {
            parse_routing(text, name.to_lowercase().contains("routing"), &mut map);
        }
        map
    }
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>, depth: u32) {
    if depth > 4 || out.len() > MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect(&path, out, depth + 1),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}

fn cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn plain(cell: &str) -> String {
    cell.chars().filter(|c| !matches!(c, '*' | '`' | '_')).collect::<String>().trim().to_lowercase()
}

fn backticked(cell: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut parts = cell.split('`');
    parts.next(); // text before the first backtick
    while let (Some(code), _) = (parts.next(), parts.next()) {
        let code = code.trim();
        let id_like = code.len() >= 2
            && code.len() <= 64
            && code.contains('-')
            && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
        if id_like {
            out.push(code.to_string());
        }
    }
    out
}

fn parse_routing(text: &str, routing_file: bool, map: &mut HashMap<String, Vec<String>>) {
    let mut routing_section = routing_file;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t.starts_with('#') {
            routing_section = routing_file || t.to_lowercase().contains("routing");
            continue;
        }
        if !t.starts_with('|') {
            continue;
        }
        // A table: header, separator, rows.
        let header = cells(t);
        let Some(sep) = lines.next() else { break };
        if !sep.contains("---") {
            continue;
        }
        let category_col = header.iter().position(|h| plain(h).contains("categor"));
        let qualifies = routing_section || category_col.is_some();
        let category_col = category_col.unwrap_or(0);
        while let Some(row) = lines.peek() {
            if !row.trim_start().starts_with('|') {
                break;
            }
            let row = cells(row);
            lines.next();
            if !qualifies {
                continue;
            }
            let Some(first) = row.get(category_col) else { continue };
            let categories: Vec<String> =
                plain(first).split(['/', ',']).map(|c| c.trim().to_string()).filter(|c| !c.is_empty() && c.len() <= 48).collect();
            for (_, cell) in row.iter().enumerate().filter(|(i, _)| *i != category_col) {
                for id in backticked(cell) {
                    let entry = map.entry(id).or_default();
                    for c in &categories {
                        if !entry.contains(c) && entry.len() < 16 {
                            entry.push(c.clone());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_skill_has_the_model_reference() {
        let s = Skill::embedded();
        assert!(s.files.iter().any(|(n, _)| n == "references/models.md"), "{:?}", s.files.iter().map(|f| &f.0).collect::<Vec<_>>());
        if let Some((first, _)) = s.files.first()
            && s.files.iter().any(|(n, _)| n == "SKILL.md")
        {
            assert_eq!(first, "SKILL.md");
        }
        assert!(s.prompt_text().contains("<skill_file path=\"references/models.md\">"));
    }

    #[test]
    fn routing_tables() {
        let text = "\
# Photo skill

| id | best at |
|---|---|
| `nano-banana-2-1` | people |

## Routing

| category | first choice | fallback |
|---|---|---|
| Portrait / headshot | `nano-banana-2-1` | `gpt-image-2` |
| **Editorial** | `nano-banana-2-1`, `wan2-7-image` | — |

## Other

| Category | model |
|---|---|
| Product | `gpt-image-2-5-sunburst` (`quality: high`) |
";
        let s = Skill { source: "t".into(), files: vec![("SKILL.md".into(), text.into())] };
        let r = s.routing();
        assert_eq!(r.get("nano-banana-2-1"), Some(&vec!["portrait".to_string(), "headshot".into(), "editorial".into()]));
        assert_eq!(r.get("gpt-image-2"), Some(&vec!["portrait".to_string(), "headshot".into()]));
        assert_eq!(r.get("wan2-7-image"), Some(&vec!["editorial".to_string()]));
        assert_eq!(r.get("gpt-image-2-5-sunburst"), Some(&vec!["product".to_string()]));
        assert!(!r.contains_key("quality: high"));

        // A routing file: every table counts; the category column may come second.
        let routing = "# Routing\n\n## People\n\n| model | categories |\n|---|---|\n| `gpt-image-2` | luxury, product |\n";
        let s = Skill { source: "t".into(), files: vec![("references/routing.md".into(), routing.into())] };
        assert_eq!(s.routing().get("gpt-image-2"), Some(&vec!["luxury".to_string(), "product".into()]));
    }

    #[test]
    fn from_dir_reads_skill_and_references() {
        let dir = std::env::temp_dir().join(format!("ps-skill-{}", crate::store::new_id()));
        std::fs::create_dir_all(dir.join("references/sub")).unwrap();
        std::fs::write(dir.join("SKILL.md"), "# Skill").unwrap();
        std::fs::write(dir.join("references/b.md"), "B").unwrap();
        std::fs::write(dir.join("references/sub/a.md"), "A").unwrap();
        std::fs::write(dir.join("references/bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        let s = Skill::from_dir(&dir).unwrap();
        let names: Vec<&str> = s.files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, vec!["SKILL.md", "references/b.md", "references/sub/a.md"]);
        assert!(Skill::from_dir(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
