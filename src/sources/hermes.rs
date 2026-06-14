use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::DateRange;
use crate::sources::{DeepMatch, SessionSource, get_snippet, matches_all_terms};

const MAX_MATCHES_PER_SESSION: usize = 2;

pub struct HermesSource;

impl HermesSource {
    fn db_path(&self) -> Option<PathBuf> {
        let db = dirs::home_dir()
            .expect("Cannot determine home directory")
            .join(".hermes")
            .join("state.db");
        if db.exists() { Some(db) } else { None }
    }
}

impl SessionSource for HermesSource {
    fn session_roots(&self) -> Vec<PathBuf> {
        self.db_path().into_iter().collect()
    }

    fn search(
        &self,
        query: &str,
        project_filter: Option<&str>,
        date_range: &DateRange,
    ) -> Vec<DeepMatch> {
        let Some(db_path) = self.db_path() else {
            return vec![];
        };
        search_hermes(&db_path, query, project_filter, date_range)
    }
}

/// Convert a Hermes `started_at` Unix timestamp (REAL) to a stable string.
/// Interpreting as seconds since epoch (standard SQLite unixepoch).
fn timestamp_to_string(ts_f64: f64) -> String {
    // SQLite REAL timestamps from Hermes are seconds since Unix epoch.
    // Format as RFC3339-like for consistency with other sources.
    let secs = ts_f64.trunc() as i64;
    let nanos = ((ts_f64 - ts_f64.trunc()) * 1_000_000_000.0).round() as u32;

    // Use chrono to format
    if let Some(dt) = chrono::DateTime::from_timestamp(secs, nanos) {
        dt.to_rfc3339()
    } else {
        ts_f64.to_string()
    }
}

/// Build a safe FTS5 query string from the user's query.
/// Wraps each term in double quotes to prevent special FTS5 operators.
fn build_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            // Escape double quotes inside the term by doubling them (FTS5 escape)
            let escaped = t.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Search Hermes sessions via FTS5 (fast path), with a LIKE fallback.
fn search_hermes(
    db_path: &Path,
    query: &str,
    project_filter: Option<&str>,
    date_range: &DateRange,
) -> Vec<DeepMatch> {
    let query_terms_lower: Vec<String> =
        query.split_whitespace().map(|s| s.to_lowercase()).collect();

    if query_terms_lower.is_empty() {
        return vec![];
    }

    // Try FTS5 fast path first
    let results = search_hermes_fts(
        db_path,
        query,
        &query_terms_lower,
        project_filter,
        date_range,
    );

    if !results.is_empty() {
        return results;
    }

    // Fallback: scan via LIKE
    search_hermes_like(
        db_path,
        query,
        &query_terms_lower,
        project_filter,
        date_range,
    )
}

fn search_hermes_fts(
    db_path: &Path,
    query: &str,
    query_terms_lower: &[String],
    project_filter: Option<&str>,
    date_range: &DateRange,
) -> Vec<DeepMatch> {
    let conn = match rusqlite::Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("WARNING: Failed to open Hermes state.db: {e}");
            return vec![];
        }
    };

    let fts_query = build_fts_query(query);

    // Build SQL with optional project filter
    let mut sql = String::from(
        r#"
        SELECT m.id, m.session_id, m.content, s.cwd, s.started_at, s.title
        FROM messages_fts f
        JOIN messages m ON f.rowid = m.id
        JOIN sessions s ON m.session_id = s.id
        WHERE messages_fts MATCH ?1
        AND s.archived = 0
        AND m.active = 1
        AND m.role IN ('user', 'assistant')
        "#,
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(fts_query)];

    if let Some(filter) = project_filter {
        sql.push_str(" AND s.cwd LIKE ?2");
        params.push(Box::new(format!("%{filter}%")));
    }

    sql.push_str(" ORDER BY m.id DESC LIMIT 500");

    // Prepare the statement with dynamic params
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("WARNING: Hermes FTS query failed: {e}");
            return vec![];
        }
    };

    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    let rows = match stmt.query_map(param_refs.as_slice(), |row| {
        let _msg_id: i64 = row.get(0)?;
        let session_id: String = row.get(1)?;
        let content: Option<String> = row.get(2)?;
        let cwd: Option<String> = row.get(3)?;
        let started_at: Option<f64> = row.get(4)?;
        let _title: Option<String> = row.get(5)?;
        Ok((
            session_id,
            content.unwrap_or_default(),
            cwd.unwrap_or_default(),
            started_at.unwrap_or(0.0),
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("WARNING: Hermes FTS query execution failed: {e}");
            return vec![];
        }
    };

    for row in rows.flatten() {
        let (session_id, content, cwd, started_at) = row;

        if content.is_empty() {
            continue;
        }

        // Dedupe: max N matches per session
        let count = seen_sessions.entry(session_id.clone()).or_insert(0);
        if *count >= MAX_MATCHES_PER_SESSION {
            continue;
        }

        // Post-filter: all query terms must appear in content
        let content_lower = content.to_lowercase();
        if !matches_all_terms(&content_lower, query_terms_lower) {
            continue;
        }

        let snippet = get_snippet(&content, query, 80);
        let timestamp = timestamp_to_string(started_at);

        if !date_range.contains(&timestamp) {
            continue;
        }

        let project_path = if cwd.is_empty() {
            "unknown".to_string()
        } else {
            cwd
        };

        matches.push(DeepMatch {
            source: "Hermes".to_string(),
            session_id: session_id.clone(),
            project_path,
            snippet,
            timestamp,
        });

        *count += 1;
    }

    matches
}

/// LIKE-based fallback when FTS5 returns no results.
fn search_hermes_like(
    db_path: &Path,
    _query: &str,
    query_terms_lower: &[String],
    project_filter: Option<&str>,
    date_range: &DateRange,
) -> Vec<DeepMatch> {
    let conn = match rusqlite::Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("WARNING: Failed to open Hermes state.db: {e}");
            return vec![];
        }
    };

    let mut sql = String::from(
        r#"
        SELECT m.id, m.session_id, m.content, s.cwd, s.started_at
        FROM messages m
        JOIN sessions s ON m.session_id = s.id
        WHERE s.archived = 0
        AND m.active = 1
        AND m.role IN ('user', 'assistant')
        AND m.content IS NOT NULL
        "#,
    );

    // Add LIKE conditions for each term
    for i in 0..query_terms_lower.len() {
        let param = format!("?{}", i + 1);
        sql.push_str(&format!(" AND LOWER(m.content) LIKE '%' || {param} || '%'"));
    }

    if project_filter.is_some() {
        let param_idx = query_terms_lower.len() + 1;
        sql.push_str(&format!(" AND s.cwd LIKE ?{param_idx}"));
    }

    sql.push_str(" ORDER BY m.id DESC");

    // Build parameter array
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for term in query_terms_lower {
        params.push(Box::new(term.clone()));
    }
    if let Some(filter) = project_filter {
        params.push(Box::new(format!("%{filter}%")));
    }

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("WARNING: Hermes LIKE query failed: {e}");
            return vec![];
        }
    };

    let mut matches = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    let rows = match stmt.query_map(param_refs.as_slice(), |row| {
        let _msg_id: i64 = row.get(0)?;
        let session_id: String = row.get(1)?;
        let content: Option<String> = row.get(2)?;
        let cwd: Option<String> = row.get(3)?;
        let started_at: Option<f64> = row.get(4)?;
        Ok((
            session_id,
            content.unwrap_or_default(),
            cwd.unwrap_or_default(),
            started_at.unwrap_or(0.0),
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("WARNING: Hermes LIKE query execution failed: {e}");
            return vec![];
        }
    };

    for row in rows.flatten() {
        let (session_id, content, cwd, started_at) = row;

        if content.is_empty() {
            continue;
        }

        let count = seen_sessions.entry(session_id.clone()).or_insert(0);
        if *count >= MAX_MATCHES_PER_SESSION {
            continue;
        }

        let snippet = get_snippet(&content, _query, 80);
        let timestamp = timestamp_to_string(started_at);

        if !date_range.contains(&timestamp) {
            continue;
        }

        let project_path = if cwd.is_empty() {
            "unknown".to_string()
        } else {
            cwd
        };

        matches.push(DeepMatch {
            source: "Hermes".to_string(),
            session_id: session_id.clone(),
            project_path,
            snippet,
            timestamp,
        });

        *count += 1;
    }

    matches
}
