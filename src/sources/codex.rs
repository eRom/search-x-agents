use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::sources::{
    get_snippet, is_ripgrep_available, matches_all_terms, parse_rg_line,
    warn_ripgrep_not_available, DeepMatch, SessionSource,
};
use crate::DateRange;

const MAX_MATCHES_PER_SESSION: usize = 2;

pub struct CodexSource;

impl SessionSource for CodexSource {
    fn session_roots(&self) -> Vec<PathBuf> {
        let dir = dirs::home_dir()
            .expect("Cannot determine home directory")
            .join(".codex");
        if dir.join("sessions").exists() {
            vec![dir]
        } else {
            vec![]
        }
    }

    fn search(
        &self,
        query: &str,
        _project_filter: Option<&str>,
        date_range: &DateRange,
    ) -> Vec<DeepMatch> {
        let roots = self.session_roots();
        if roots.is_empty() {
            return vec![];
        }
        search_codex(query, &roots[0], date_range)
    }
}

/// Extract text from Codex response_item message
/// Format: {timestamp, type: "response_item", payload: {type: "message", role, content: [{type: "input_text"/"output_text", text}]}}
fn extract_text_codex(value: &serde_json::Value) -> (String, String) {
    let payload = match value.get("payload") {
        Some(p) => p,
        None => return (String::new(), String::new()),
    };

    if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
        return (String::new(), String::new());
    }

    let role = payload
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();

    let content = match payload.get("content") {
        Some(c) => c,
        None => return (role, String::new()),
    };

    let text = match content {
        serde_json::Value::Array(arr) => {
            let mut texts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    texts.push(text.to_string());
                }
            }
            texts.join(" ")
        }
        _ => content.to_string(),
    };

    (role, text)
}

/// Load session metadata by reading first line of each session file
fn load_codex_metadata(base: &Path) -> HashMap<String, (String, String)> {
    let mut metadata = HashMap::new(); // session_id -> (cwd, timestamp)
    let sessions_dir = base.join("sessions");

    fn walk(dir: &Path, meta: &mut HashMap<String, (String, String)>) {
        let Ok(entries) = fs::read_dir(dir) else {
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
                walk(&path, meta);
            } else if file_type.is_file()
                && path.extension().is_some_and(|e| e == "jsonl")
            {
                let session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if session_id.is_empty() || meta.contains_key(&session_id) {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&path)
                    && let Some(first_line) = content.lines().next()
                    && let Ok(record) = serde_json::from_str::<serde_json::Value>(first_line)
                {
                    if record.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
                        let cwd = record
                            .get("payload")
                            .and_then(|p| p.get("cwd"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
                        let timestamp = record
                            .get("timestamp")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        meta.insert(session_id, (cwd, timestamp));
                    }
                }
            }
        }
    }

    if sessions_dir.exists() {
        walk(&sessions_dir, &mut metadata);
    }
    metadata
}

fn search_codex(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    if !is_ripgrep_available() {
        return search_codex_rust(query, base, date_range);
    }

    let sessions_dir = base.join("sessions");
    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let metadata = load_codex_metadata(base);

    let output = Command::new("rg")
        .args([
            "--no-heading",
            "--line-number",
            "--ignore-case",
            "--glob",
            "*.jsonl",
            query,
        ])
        .arg(&sessions_dir)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("WARNING: Failed to run ripgrep: {e}. Using Rust fallback.");
            return search_codex_rust(query, base, date_range);
        }
    };

    if !output.status.success() && output.status.code() != Some(1) {
        eprintln!(
            "WARNING: ripgrep returned unexpected exit code: {:?}",
            output.status.code()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    for line in stdout.lines() {
        let (path, record) = match parse_rg_line(line) {
            Some(r) => r,
            None => continue,
        };

        // Only process response_item of type message
        if record.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let count = seen_sessions.entry(session_id.clone()).or_insert(0);
        if *count >= MAX_MATCHES_PER_SESSION {
            continue;
        }

        let (role, text) = extract_text_codex(&record);
        if text.is_empty() || (role != "user" && role != "assistant") {
            continue;
        }

        let text_lower = text.to_lowercase();
        if !matches_all_terms(&text_lower, &query_terms_lower) {
            continue;
        }

        let snippet = get_snippet(&text, query, 80);

        let timestamp = record
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if !date_range.contains(&timestamp) {
            continue;
        }

        let (project_path, _) = metadata
            .get(&session_id)
            .cloned()
            .unwrap_or_else(|| ("unknown".to_string(), String::new()));

        matches.push(DeepMatch {
            source: "Codex".to_string(),
            session_id: session_id.clone(),
            project_path,
            snippet,
            timestamp,
        });

        *count += 1;
    }

    matches
}

fn search_codex_rust(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    warn_ripgrep_not_available();

    let sessions_dir = base.join("sessions");
    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let metadata = load_codex_metadata(base);

    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    fn walk_search(
        dir: &Path,
        query_terms_lower: &[String],
        date_range: &DateRange,
        metadata: &HashMap<String, (String, String)>,
        matches: &mut Vec<DeepMatch>,
        seen_sessions: &mut HashMap<String, usize>,
        query: &str,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
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
                walk_search(
                    &path,
                    query_terms_lower,
                    date_range,
                    metadata,
                    matches,
                    seen_sessions,
                    query,
                );
            } else if file_type.is_file()
                && path.extension().is_some_and(|e| e == "jsonl")
            {
                let session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let Ok(file) = std::fs::File::open(&path) else {
                    continue;
                };
                let reader = std::io::BufReader::new(file);

                for line in reader.lines() {
                    let Ok(line) = line else { continue };
                    let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };

                    if record.get("type").and_then(|t| t.as_str()) != Some("response_item") {
                        continue;
                    }

                    let count = seen_sessions.entry(session_id.clone()).or_insert(0);
                    if *count >= MAX_MATCHES_PER_SESSION {
                        break;
                    }

                    let (role, text) = extract_text_codex(&record);
                    if text.is_empty() || (role != "user" && role != "assistant") {
                        continue;
                    }

                    let text_lower = text.to_lowercase();
                    if !matches_all_terms(&text_lower, query_terms_lower) {
                        continue;
                    }

                    let snippet = get_snippet(&text, query, 80);

                    let timestamp = record
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !date_range.contains(&timestamp) {
                        continue;
                    }

                    let (project_path, _) = metadata
                        .get(&session_id)
                        .cloned()
                        .unwrap_or_else(|| ("unknown".to_string(), String::new()));

                    matches.push(DeepMatch {
                        source: "Codex".to_string(),
                        session_id: session_id.clone(),
                        project_path,
                        snippet,
                        timestamp,
                    });

                    *count += 1;
                }
            }
        }
    }

    if sessions_dir.exists() {
        walk_search(
            &sessions_dir,
            &query_terms_lower,
            date_range,
            &metadata,
            &mut matches,
            &mut seen_sessions,
            query,
        );
    }

    matches
}
