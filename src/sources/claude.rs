use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};

use crate::DateRange;
use crate::sources::{
    DeepMatch, SessionSource, extract_content_array, find_jsonl_files, get_snippet,
    is_ripgrep_available, matches_all_terms, parse_rg_line, session_id_from_path,
    warn_ripgrep_not_available,
};

const MAX_MATCHES_PER_SESSION: usize = 2;

/// Index file inside each `memory/` dir; redundant pointers, skipped to avoid noise.
const MEMORY_INDEX_FILE: &str = "MEMORY.md";
/// Cap on jsonl lines scanned when resolving a project's real cwd for memory hits.
const MEMORY_MAX_HEADER_LINES: usize = 50;

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
        let mut matches = search_claude(query, project_filter, &roots[0], date_range);
        matches.extend(search_claude_memory(
            query,
            project_filter,
            &roots[0],
            date_range,
        ));
        matches
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

// ─── Memory Search ──────────────────────────────────────────────────

/// Search the persistent memory notes Claude keeps per project at
/// `~/.claude/projects/<slug>/memory/*.md`. Unlike sessions, AND-matching is
/// done over the whole document (terms may span lines), so this is a plain
/// Rust scan rather than a line-oriented ripgrep call.
fn search_claude_memory(
    query: &str,
    project_filter: Option<&str>,
    projects_base: &Path,
    date_range: &DateRange,
) -> Vec<DeepMatch> {
    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let mut matches = Vec::new();

    for memory_dir in memory_dirs(projects_base, project_filter) {
        let Ok(entries) = fs::read_dir(&memory_dir) else {
            continue;
        };
        // Resolved lazily on the first hit, then reused for the whole dir.
        let mut project_path: Option<String> = None;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            if path.file_name().is_some_and(|n| n == MEMORY_INDEX_FILE) {
                continue;
            }

            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            if !matches_all_terms(&content.to_lowercase(), &query_terms_lower) {
                continue;
            }

            let timestamp = memory_timestamp(&path);
            if !date_range.contains(&timestamp) {
                continue;
            }

            let project_path = project_path
                .get_or_insert_with(|| project_path_for_memory(&memory_dir))
                .clone();

            matches.push(DeepMatch {
                source: "Claude Memory".to_string(),
                session_id: session_id_from_path(&path),
                project_path,
                snippet: get_snippet(&content, query, 80),
                timestamp,
            });
        }
    }

    matches
}

/// The `memory/` directories to scan, one per project, honoring `project_filter`
/// with the same substring semantics as session search.
fn memory_dirs(projects_base: &Path, project_filter: Option<&str>) -> Vec<PathBuf> {
    let filter_lower = project_filter.map(str::to_lowercase);
    let mut dirs = Vec::new();

    let Ok(entries) = fs::read_dir(projects_base) else {
        return dirs;
    };
    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        if let Some(ref filter) = filter_lower
            && !project_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase().contains(filter))
                .unwrap_or(false)
        {
            continue;
        }
        let memory_dir = project_dir.join("memory");
        if memory_dir.is_dir() {
            dirs.push(memory_dir);
        }
    }

    dirs
}

/// Project path for a memory hit: the real `cwd` recorded in a sibling session
/// file (so it matches how the same project's session hits are displayed),
/// falling back to the encoded slug directory name.
fn project_path_for_memory(memory_dir: &Path) -> String {
    let Some(project_dir) = memory_dir.parent() else {
        return "unknown".to_string();
    };
    if let Some(cwd) = first_cwd_in_dir(project_dir) {
        return cwd;
    }
    project_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

/// First non-empty `cwd` found in any top-level `.jsonl` of `project_dir`.
fn first_cwd_in_dir(project_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(project_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let reader = std::io::BufReader::new(file);
        for line in reader
            .lines()
            .take(MEMORY_MAX_HEADER_LINES)
            .map_while(Result::ok)
        {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if let Some(cwd) = record
                .get("cwd")
                .and_then(|c| c.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// File modification time as an RFC3339 string, compatible with `DateRange`.
fn memory_timestamp(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
        .unwrap_or_default()
}
