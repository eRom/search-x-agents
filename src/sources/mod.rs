use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;


use crate::DateRange;

pub mod claude;
pub mod openclaw;
pub mod pi;
pub mod codex;
pub mod antigravity;
pub mod cowork;
pub mod hermes;

const MAX_SNIPPET_LEN: usize = 200;

// ─── Data Structures ────────────────────────────────────────────────

pub struct DeepMatch {
    pub source: String,
    pub session_id: String,
    pub project_path: String,
    pub snippet: String,
    pub timestamp: String,
}

// ─── Trait ──────────────────────────────────────────────────────────

pub trait SessionSource {
    fn session_roots(&self) -> Vec<PathBuf>;
    fn search(
        &self,
        query: &str,
        project_filter: Option<&str>,
        date_range: &DateRange,
    ) -> Vec<DeepMatch>;
}

// ─── Helpers ────────────────────────────────────────────────────────

pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        s.chars().take(max_len).collect()
    }
}

pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

pub fn get_snippet(text: &str, query: &str, context_chars: usize) -> String {
    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();

    let mut idx = text_lower.find(&query_lower);
    if idx.is_none() {
        for term in query.split_whitespace() {
            idx = text_lower.find(&term.to_lowercase());
            if idx.is_some() {
                break;
            }
        }
    }

    let idx = match idx {
        Some(i) => i,
        None => return truncate(text, MAX_SNIPPET_LEN),
    };

    let start = idx.saturating_sub(context_chars);
    let end = (idx + query.len() + context_chars).min(text.len());
    let start = floor_char_boundary(text, start);
    let end = ceil_char_boundary(text, end);

    let snippet = &text[start..end];
    let mut result = String::new();
    if start > 0 {
        result.push_str("...");
    }
    result.push_str(snippet);
    if end < text.len() {
        result.push_str("...");
    }
    result
}

pub fn matches_all_terms(text_lower: &str, query_terms_lower: &[String]) -> bool {
    query_terms_lower
        .iter()
        .all(|term| text_lower.contains(term))
}

pub fn extract_content_array(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let mut texts = Vec::new();
            for item in arr {
                if let Some(t) = item.get("type").and_then(|t| t.as_str()) {
                    match t {
                        "text" => {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                texts.push(text.to_string());
                            }
                        }
                        "tool_result" => {
                            if let Some(c) = item.get("content") {
                                texts.push(c.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            texts.join(" ")
        }
        _ => content.to_string(),
    }
}

// ─── Ripgrep Helpers ────────────────────────────────────────────────

static RIPGREP_AVAILABLE: OnceLock<bool> = OnceLock::new();

pub fn is_ripgrep_available() -> bool {
    *RIPGREP_AVAILABLE.get_or_init(|| {
        Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

static RIPGREP_WARNING_SHOWN: OnceLock<()> = OnceLock::new();

pub fn warn_ripgrep_not_available() {
    RIPGREP_WARNING_SHOWN.get_or_init(|| {
        eprintln!("WARNING: ripgrep (rg) not found. Using slower Rust fallback.");
        eprintln!("         Install ripgrep for 3-5x faster search: brew install ripgrep");
        eprintln!();
    });
}

pub fn parse_rg_line(line: &str) -> Option<(PathBuf, serde_json::Value)> {
    let first_colon = line.find(':')?;
    let path = PathBuf::from(&line[..first_colon]);
    let rest = &line[first_colon + 1..];
    let second_colon = rest.find(':')?;
    let json_str = &rest[second_colon + 1..];
    let value = serde_json::from_str(json_str).ok()?;
    Some((path, value))
}

pub fn find_jsonl_files(base: &Path, exclude_subagents: bool, exclude_deleted: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();

    fn walk_dir(
        dir: &Path,
        files: &mut Vec<PathBuf>,
        exclude_subagents: bool,
        exclude_deleted: bool,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }

            if file_type.is_dir() {
                if exclude_subagents && path.file_name().is_some_and(|n| n == "subagents") {
                    continue;
                }
                walk_dir(&path, files, exclude_subagents, exclude_deleted);
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                if exclude_deleted && path.to_string_lossy().contains(".deleted.") {
                    continue;
                }
                if path.file_name().is_some_and(|n| n == "sessions-index.json") {
                    continue;
                }
                files.push(path);
            }
        }
    }

    walk_dir(base, &mut files, exclude_subagents, exclude_deleted);
    files
}

pub fn session_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

// ─── Dispatch ───────────────────────────────────────────────────────

pub fn search_all_sources(
    query: &str,
    sources: &[Box<dyn SessionSource + '_>],
    project_filter: Option<&str>,
    date_range: &DateRange,
) -> Vec<DeepMatch> {
    let mut all_matches: Vec<DeepMatch> = Vec::new();
    for source in sources {
        let matches = source.search(query, project_filter, date_range);
        all_matches.extend(matches);
    }
    // Sort by timestamp descending
    all_matches.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all_matches
}
