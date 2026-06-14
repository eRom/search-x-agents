use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::DateRange;
use crate::sources::{
    DeepMatch, SessionSource, get_snippet, is_ripgrep_available, matches_all_terms, parse_rg_line,
    warn_ripgrep_not_available,
};

const MAX_MATCHES_PER_SESSION: usize = 2;

pub struct AntigravitySource;

impl SessionSource for AntigravitySource {
    fn session_roots(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().expect("Cannot determine home directory");
        let mut roots = Vec::new();
        for sub in &["antigravity", "antigravity-ide", "antigravity-cli"] {
            let dir = home.join(".gemini").join(sub).join("brain");
            if dir.exists() {
                roots.push(dir);
            }
        }
        roots
    }

    fn search(
        &self,
        query: &str,
        _project_filter: Option<&str>,
        date_range: &DateRange,
    ) -> Vec<DeepMatch> {
        let mut all_matches = Vec::new();
        for root in self.session_roots() {
            let matches = search_antigravity(query, &root, date_range);
            all_matches.extend(matches);
        }
        all_matches.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all_matches
    }
}

/// Extract text from Antigravity transcript entry
/// Format: {step_index, source, type, content, created_at, tool_calls: [{name, args}]}
fn extract_text_antigravity(value: &serde_json::Value) -> String {
    let mut texts = Vec::new();

    // Main content
    if let Some(content) = value.get("content").and_then(|c| c.as_str()) {
        texts.push(content.to_string());
    }

    // Tool calls args (contain useful text like file contents, commands)
    if let Some(tool_calls) = value.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tool_calls {
            if let Some(args) = tc.get("args")
                && let Some(obj) = args.as_object()
            {
                for (_key, val) in obj {
                    if let Some(s) = val.as_str() {
                        texts.push(s.to_string());
                    }
                }
            }
        }
    }

    texts.join(" ")
}

fn search_antigravity(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    if !is_ripgrep_available() {
        return search_antigravity_rust(query, base, date_range);
    }

    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();

    // Prefer transcript_full.jsonl, fallback to transcript.jsonl
    let glob_pattern = "transcript*.jsonl";

    let output = Command::new("rg")
        .args([
            "--no-heading",
            "--line-number",
            "--ignore-case",
            "--hidden",
            "--glob",
            glob_pattern,
            "--glob",
            "!**/.obsidian/**",
            "--glob",
            "!**/.git/**",
            "--glob",
            "!**/node_modules/**",
            query,
        ])
        .arg(base)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("WARNING: Failed to run ripgrep: {e}. Using Rust fallback.");
            return search_antigravity_rust(query, base, date_range);
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

        // Session ID is the UUID directory two levels above the transcript file
        // path: <brain>/<uuid>/.system_generated/logs/transcript.jsonl
        let session_id = path
            .parent() // logs/
            .and_then(|p| p.parent()) // .system_generated/
            .and_then(|p| p.parent()) // <uuid>/
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let count = seen_sessions.entry(session_id.clone()).or_insert(0);
        if *count >= MAX_MATCHES_PER_SESSION {
            continue;
        }

        let text = extract_text_antigravity(&record);
        if text.is_empty() {
            continue;
        }

        let text_lower = text.to_lowercase();
        if !matches_all_terms(&text_lower, &query_terms_lower) {
            continue;
        }

        let snippet = get_snippet(&text, query, 80);

        let timestamp = record
            .get("created_at")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if !date_range.contains(&timestamp) {
            continue;
        }

        matches.push(DeepMatch {
            source: "Antigravity".to_string(),
            session_id: session_id.clone(),
            project_path: "unknown".to_string(),
            snippet,
            timestamp,
        });

        *count += 1;
    }

    matches
}

fn search_antigravity_rust(query: &str, base: &Path, date_range: &DateRange) -> Vec<DeepMatch> {
    warn_ripgrep_not_available();

    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();

    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    // Walk: <brain>/<uuid>/.system_generated/logs/transcript.jsonl
    fn walk_search(
        base: &Path,
        query_terms_lower: &[String],
        date_range: &DateRange,
        matches: &mut Vec<DeepMatch>,
        seen_sessions: &mut HashMap<String, usize>,
        query: &str,
    ) {
        let Ok(entries) = fs::read_dir(base) else {
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
                // Skip hidden dirs except .system_generated
                let is_hidden = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                if is_hidden && path.file_name().is_some_and(|n| n != ".system_generated") {
                    continue;
                }
                walk_search(
                    &path,
                    query_terms_lower,
                    date_range,
                    matches,
                    seen_sessions,
                    query,
                );
            } else if file_type.is_file()
                && path.extension().is_some_and(|e| e == "jsonl")
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("transcript"))
            {
                let session_id = path
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
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

                    let count = seen_sessions.entry(session_id.clone()).or_insert(0);
                    if *count >= MAX_MATCHES_PER_SESSION {
                        break;
                    }

                    let text = extract_text_antigravity(&record);
                    if text.is_empty() {
                        continue;
                    }

                    let text_lower = text.to_lowercase();
                    if !matches_all_terms(&text_lower, query_terms_lower) {
                        continue;
                    }

                    let snippet = get_snippet(&text, query, 80);

                    let timestamp = record
                        .get("created_at")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !date_range.contains(&timestamp) {
                        continue;
                    }

                    matches.push(DeepMatch {
                        source: "Antigravity".to_string(),
                        session_id: session_id.clone(),
                        project_path: "unknown".to_string(),
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
        &mut matches,
        &mut seen_sessions,
        query,
    );
    matches
}
