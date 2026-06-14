use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::DateRange;
use crate::sources::{
    DeepMatch, SessionSource, extract_content_array, find_jsonl_files, get_snippet,
    is_ripgrep_available, matches_all_terms, parse_rg_line, warn_ripgrep_not_available,
};

const MAX_MATCHES_PER_SESSION: usize = 2;

pub struct ClaudeSource;

impl SessionSource for ClaudeSource {
    fn session_roots(&self) -> Vec<PathBuf> {
        let dir = dirs::home_dir()
            .expect("Cannot determine home directory")
            .join(".claude")
            .join("projects");
        if dir.exists() { vec![dir] } else { vec![] }
    }

    fn search(
        &self,
        query: &str,
        project_filter: Option<&str>,
        date_range: &DateRange,
    ) -> Vec<DeepMatch> {
        let roots = self.session_roots();
        if roots.is_empty() {
            return vec![];
        }
        search_claude(query, project_filter, &roots[0], date_range)
    }
}

fn resolve_search_path(base: &Path, project_filter: Option<&str>) -> PathBuf {
    if let Some(filter) = project_filter {
        let filter_lower = filter.to_lowercase();
        if let Ok(entries) = fs::read_dir(base) {
            for entry in entries.flatten() {
                if entry.path().is_dir()
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&filter_lower)
                {
                    return entry.path();
                }
            }
        }
    }
    base.to_path_buf()
}

fn extract_text_claude(value: &serde_json::Value) -> String {
    let Some(message) = value.get("message") else {
        return String::new();
    };
    let Some(content) = message.get("content") else {
        return String::new();
    };
    extract_content_array(content)
}

fn search_claude(
    query: &str,
    project_filter: Option<&str>,
    base: &Path,
    date_range: &DateRange,
) -> Vec<DeepMatch> {
    if !is_ripgrep_available() {
        return search_claude_rust(query, project_filter, base, date_range);
    }

    let search_path = resolve_search_path(base, project_filter);
    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();

    let output = Command::new("rg")
        .args([
            "--no-heading",
            "--line-number",
            "--ignore-case",
            "--glob",
            "*.jsonl",
            "--glob",
            "!**/subagents/**",
            "--glob",
            "!**/sessions-index.json",
            query,
        ])
        .arg(&search_path)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("WARNING: Failed to run ripgrep: {e}. Using Rust fallback.");
            return search_claude_rust(query, project_filter, base, date_range);
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
        let (_path, record) = match parse_rg_line(line) {
            Some(r) => r,
            None => continue,
        };

        let record_type = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if record_type != "user" && record_type != "assistant" {
            continue;
        }

        let session_id = record
            .get("sessionId")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();

        let count = seen_sessions.entry(session_id.clone()).or_insert(0);
        if *count >= MAX_MATCHES_PER_SESSION {
            continue;
        }

        let text = extract_text_claude(&record);
        if text.is_empty() {
            continue;
        }

        let text_lower = text.to_lowercase();
        if !matches_all_terms(&text_lower, &query_terms_lower) {
            continue;
        }

        let snippet = get_snippet(&text, query, 80);

        let project_path = record
            .get("cwd")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| "unknown".to_string());

        let timestamp = record
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if !date_range.contains(&timestamp) {
            continue;
        }

        matches.push(DeepMatch {
            source: "Claude Code".to_string(),
            session_id: session_id.clone(),
            project_path,
            snippet,
            timestamp,
        });

        *count += 1;
    }

    matches
}

fn search_claude_rust(
    query: &str,
    project_filter: Option<&str>,
    base: &Path,
    date_range: &DateRange,
) -> Vec<DeepMatch> {
    warn_ripgrep_not_available();

    let search_path = resolve_search_path(base, project_filter);
    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();

    let jsonl_files = find_jsonl_files(&search_path, true, false);
    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    for file_path in jsonl_files {
        let Ok(file) = std::fs::File::open(&file_path) else {
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

            let session_id = record
                .get("sessionId")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();

            let count = seen_sessions.entry(session_id.clone()).or_insert(0);
            if *count >= MAX_MATCHES_PER_SESSION {
                continue;
            }

            let text = extract_text_claude(&record);
            if text.is_empty() {
                continue;
            }

            let text_lower = text.to_lowercase();
            if !matches_all_terms(&text_lower, &query_terms_lower) {
                continue;
            }

            let snippet = get_snippet(&text, query, 80);

            let project_path = record
                .get("cwd")
                .and_then(|c| c.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| "unknown".to_string());

            let timestamp = record
                .get("timestamp")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            if !date_range.contains(&timestamp) {
                continue;
            }

            matches.push(DeepMatch {
                source: "Claude Code".to_string(),
                session_id: session_id.clone(),
                project_path,
                snippet,
                timestamp,
            });

            *count += 1;
        }
    }

    matches
}
