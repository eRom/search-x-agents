use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::DateRange;
use crate::sources::{
    DeepMatch, SessionSource, extract_content_array, find_jsonl_files, get_snippet,
    is_ripgrep_available, matches_all_terms, parse_rg_line, session_id_from_path,
    warn_ripgrep_not_available,
};

const MAX_MATCHES_PER_SESSION: usize = 2;

struct PiSessionMeta {
    cwd: String,
    timestamp: String,
}

pub struct PiSource;

impl SessionSource for PiSource {
    fn session_roots(&self) -> Vec<PathBuf> {
        let dir = dirs::home_dir()
            .expect("Cannot determine home directory")
            .join(".pi")
            .join("agent")
            .join("sessions");
        if dir.exists() { vec![dir] } else { vec![] }
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
        search_pi(query, &roots[0], date_range)
    }
}

fn extract_text_pi(value: &serde_json::Value) -> (String, String) {
    let Some(message) = value.get("message") else {
        return (String::new(), String::new());
    };
    let role = message
        .get("role")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let Some(content) = message.get("content") else {
        return (role, String::new());
    };
    (role, extract_content_array(content))
}

fn load_pi_session_metadata(base: &Path) -> HashMap<String, PiSessionMeta> {
    let mut metadata = HashMap::new();
    let Ok(projects) = fs::read_dir(base) else {
        return metadata;
    };
    for project_entry in projects.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let Ok(sessions) = fs::read_dir(&project_path) else {
            continue;
        };
        for session_entry in sessions.flatten() {
            let session_path = session_entry.path();
            if session_path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let session_id = session_id_from_path(&session_path);
            if session_id.is_empty() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&session_path)
                && let Some(first_line) = content.lines().next()
                && let Ok(record) = serde_json::from_str::<serde_json::Value>(first_line)
            {
                let record_type = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if record_type == "session" {
                    let cwd = record
                        .get("cwd")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let timestamp = record
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    metadata.insert(session_id, PiSessionMeta { cwd, timestamp });
                }
            }
        }
    }
    metadata
}

fn search_pi(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    if !is_ripgrep_available() {
        return search_pi_rust(query, base, date_range);
    }

    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let session_metadata = load_pi_session_metadata(base);

    let output = Command::new("rg")
        .args([
            "--no-heading",
            "--line-number",
            "--ignore-case",
            "--glob",
            "*.jsonl",
            query,
        ])
        .arg(base)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("WARNING: Failed to run ripgrep: {e}. Using Rust fallback.");
            return search_pi_rust(query, base, date_range);
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
        if record_type != "message" {
            continue;
        }

        let session_id = session_id_from_path(&path);
        let count = seen_sessions.entry(session_id.clone()).or_insert(0);
        if *count >= MAX_MATCHES_PER_SESSION {
            continue;
        }

        let (role, text) = extract_text_pi(&record);
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
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                session_metadata
                    .get(&session_id)
                    .map(|m| m.timestamp.clone())
            })
            .unwrap_or_default();

        let project_path = session_metadata
            .get(&session_id)
            .map(|m| m.cwd.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string());

        if !date_range.contains(&timestamp) {
            continue;
        }

        matches.push(DeepMatch {
            source: "Pi".to_string(),
            session_id: session_id.clone(),
            project_path,
            snippet,
            timestamp,
        });

        *count += 1;
    }

    matches
}

fn search_pi_rust(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    warn_ripgrep_not_available();

    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let session_metadata = load_pi_session_metadata(base);
    let jsonl_files = find_jsonl_files(base, false, false);

    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    for file_path in jsonl_files {
        let Ok(file) = std::fs::File::open(&file_path) else {
            continue;
        };
        let reader = std::io::BufReader::new(file);
        let session_id = session_id_from_path(&file_path);

        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };

            let record_type = record.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if record_type != "message" {
                continue;
            }

            let count = seen_sessions.entry(session_id.clone()).or_insert(0);
            if *count >= MAX_MATCHES_PER_SESSION {
                continue;
            }

            let (role, text) = extract_text_pi(&record);
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
                .filter(|s| !s.is_empty())
                .map(String::from)
                .or_else(|| {
                    session_metadata
                        .get(&session_id)
                        .map(|m| m.timestamp.clone())
                })
                .unwrap_or_default();

            let project_path = session_metadata
                .get(&session_id)
                .map(|m| m.cwd.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string());

            if !date_range.contains(&timestamp) {
                continue;
            }

            matches.push(DeepMatch {
                source: "Pi".to_string(),
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
