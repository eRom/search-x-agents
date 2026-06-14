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

pub struct CoworkSource;

impl SessionSource for CoworkSource {
    fn session_roots(&self) -> Vec<PathBuf> {
        let dir = dirs::home_dir()
            .expect("Cannot determine home directory")
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("local-agent-mode-sessions");
        if dir.exists() {
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
        search_cowork(query, &roots[0], date_range)
    }
}

/// Extract text from Cowork audit.jsonl message
/// Format: {type: "user"|"assistant", message: {role, content: [{type: "text"|"tool_use"|"tool_result"}]}}
fn extract_text_cowork(value: &serde_json::Value) -> (String, String) {
    let msg_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

    let message = match value.get("message") {
        Some(m) => m,
        None => return (msg_type.to_string(), String::new()),
    };

    let role = message
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();

    let content = match message.get("content") {
        Some(c) => c,
        None => return (role, String::new()),
    };

    // Cowork uses Anthropic API content blocks: text, tool_use, tool_result, thinking
    let text = match content {
        serde_json::Value::Array(arr) => {
            let mut texts = Vec::new();
            for item in arr {
                let block_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "text" => {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            texts.push(t.to_string());
                        }
                    }
                    "tool_use" => {
                        if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                            texts.push(format!("[tool:{name}]"));
                        }
                        if let Some(input) = item.get("input") {
                            texts.push(input.to_string());
                        }
                    }
                    "tool_result" => {
                        if let Some(c) = item.get("content") {
                            match c {
                                serde_json::Value::String(s) => texts.push(s.clone()),
                                serde_json::Value::Array(arr) => {
                                    for sub in arr {
                                        if let Some(t) = sub.get("text").and_then(|t| t.as_str()) {
                                            texts.push(t.to_string());
                                        }
                                    }
                                }
                                _ => texts.push(c.to_string()),
                            }
                        }
                    }
                    _ => {}
                }
            }
            texts.join(" ")
        }
        _ => content.to_string(),
    };

    (role, text)
}

/// Load project path from local_<UUID>.json metadata files
fn load_cowork_metadata(base: &Path) -> HashMap<String, String> {
    let mut metadata = HashMap::new(); // session_id -> project_path

    fn walk(dir: &Path, meta: &mut HashMap<String, String>) {
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
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("local_"))
                && path.extension().is_some_and(|e| e == "json")
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
                    && let Ok(record) = serde_json::from_str::<serde_json::Value>(&content)
                {
                    let cwd = record
                        .get("userSelectedFolders")
                        .and_then(|f| f.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    meta.insert(session_id, cwd);
                }
            }
        }
    }

    walk(base, &mut metadata);
    metadata
}

fn search_cowork(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    if !is_ripgrep_available() {
        return search_cowork_rust(query, base, date_range);
    }

    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let metadata = load_cowork_metadata(base);

    let output = Command::new("rg")
        .args([
            "--no-heading",
            "--line-number",
            "--ignore-case",
            "--glob",
            "audit.jsonl",
            query,
        ])
        .arg(base)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("WARNING: Failed to run ripgrep: {e}. Using Rust fallback.");
            return search_cowork_rust(query, base, date_range);
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

        let record_type = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if record_type != "user" && record_type != "assistant" {
            continue;
        }

        // Session ID is the local_<UUID> directory name
        let session_id = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let count = seen_sessions.entry(session_id.clone()).or_insert(0);
        if *count >= MAX_MATCHES_PER_SESSION {
            continue;
        }

        let (_role, text) = extract_text_cowork(&record);
        if text.is_empty() {
            continue;
        }

        let text_lower = text.to_lowercase();
        if !matches_all_terms(&text_lower, &query_terms_lower) {
            continue;
        }

        let snippet = get_snippet(&text, query, 80);

        let timestamp = record
            .get("_audit_timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if !date_range.contains(&timestamp) {
            continue;
        }

        let project_path = metadata
            .get(&session_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        matches.push(DeepMatch {
            source: "Cowork".to_string(),
            session_id: session_id.clone(),
            project_path,
            snippet,
            timestamp,
        });

        *count += 1;
    }

    matches
}

fn search_cowork_rust(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    warn_ripgrep_not_available();

    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let metadata = load_cowork_metadata(base);

    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    fn walk_search(
        dir: &Path,
        query_terms_lower: &[String],
        date_range: &DateRange,
        metadata: &HashMap<String, String>,
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
                && path.file_name().is_some_and(|n| n == "audit.jsonl")
            {
                let session_id = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
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
                    let record_type = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if record_type != "user" && record_type != "assistant" {
                        continue;
                    }

                    let count = seen_sessions.entry(session_id.clone()).or_insert(0);
                    if *count >= MAX_MATCHES_PER_SESSION {
                        break;
                    }

                    let (_role, text) = extract_text_cowork(&record);
                    if text.is_empty() {
                        continue;
                    }

                    let text_lower = text.to_lowercase();
                    if !matches_all_terms(&text_lower, query_terms_lower) {
                        continue;
                    }

                    let snippet = get_snippet(&text, query, 80);

                    let timestamp = record
                        .get("_audit_timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !date_range.contains(&timestamp) {
                        continue;
                    }

                    let project_path = metadata
                        .get(&session_id)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());

                    matches.push(DeepMatch {
                        source: "Cowork".to_string(),
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

    walk_search(
        base,
        &query_terms_lower,
        date_range,
        &metadata,
        &mut matches,
        &mut seen_sessions,
        query,
    );
    matches
}
