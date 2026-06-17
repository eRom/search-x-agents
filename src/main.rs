mod sources;

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use clap::Parser;
use serde::Serialize;

use crate::sources::{
    DeepMatch, SessionSource, antigravity::AntigravitySource, claude::ClaudeSource,
    codex::CodexSource, cowork::CoworkSource, hermes::HermesSource, openclaw::OpenClawSource,
    pi::PiSource, search_all_sources,
};

// ─── Constants ──────────────────────────────────────────────────────

const DEFAULT_LIMIT: usize = 20;

// ─── CLI ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "search-x-agents",
    about = "Search across all AI agent session histories"
)]
struct Cli {
    /// Search query (words are ANDed together)
    query: Vec<String>,

    /// Search Claude Code sessions (~/.claude/projects/)
    #[arg(long)]
    claude: bool,

    /// Search OpenClaw sessions (~/.openclaw/agents/<agent>/sessions/)
    #[arg(long)]
    openclaw: bool,

    /// Search Codex sessions (~/.codex/sessions/)
    #[arg(long)]
    codex: bool,

    /// Search Pi sessions (~/.pi/agent/sessions/)
    #[arg(long)]
    pi: bool,

    /// Search Antigravity sessions (~/.gemini/antigravity*/brain/)
    #[arg(long)]
    antigravity: bool,

    /// Search Cowork sessions (~/Library/Application Support/Claude/local-agent-mode-sessions/)
    #[arg(long)]
    cowork: bool,

    /// Search Hermes sessions (~/.hermes/state.db)
    #[arg(long)]
    hermes: bool,

    /// Maximum results to show
    #[arg(long, default_value_t = DEFAULT_LIMIT)]
    limit: usize,

    /// Filter to sessions from projects matching this substring
    #[arg(long)]
    project: Option<String>,

    /// OpenClaw agent to search (default: main)
    #[arg(long, default_value = "main")]
    agent: String,

    /// Filter results from this date/time
    #[arg(long)]
    since: Option<String>,

    /// Filter results until this date/time
    #[arg(long)]
    until: Option<String>,

    /// Shorthand for --since <date> --until <date+1day>
    #[arg(long)]
    date: Option<String>,
}

// ─── Date Range Filtering ───────────────────────────────────────────

#[derive(Clone)]
pub struct DateRange {
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

impl DateRange {
    pub fn contains(&self, timestamp_str: &str) -> bool {
        if self.since.is_none() && self.until.is_none() {
            return true;
        }
        let Some(dt) = parse_timestamp(timestamp_str) else {
            return false;
        };
        if let Some(ref since) = self.since
            && dt < *since
        {
            return false;
        }
        if let Some(ref until) = self.until
            && dt >= *until
        {
            return false;
        }
        true
    }
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.to_utc());
    }
    if s.ends_with('z') {
        let mut fixed = s.to_string();
        fixed.pop();
        fixed.push('Z');
        if let Ok(dt) = DateTime::parse_from_rfc3339(&fixed) {
            return Some(dt.to_utc());
        }
    }
    None
}

fn unit_to_delta(unit: &str, n: i64) -> Option<TimeDelta> {
    match unit {
        "day" => Some(TimeDelta::days(n)),
        "week" => Some(TimeDelta::weeks(n)),
        "month" => Some(TimeDelta::days(n * 30)),
        _ => None,
    }
}

fn start_of_day(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0).unwrap().and_utc()
}

fn try_parse_ago(s: &str, today: NaiveDate) -> Option<DateTime<Utc>> {
    let rest = s.strip_suffix(" ago")?;
    let mut parts = rest.split_whitespace();
    let n: i64 = parts.next()?.parse().ok()?;
    let unit = parts.next()?.trim_end_matches('s');
    if parts.next().is_some() {
        return None;
    }
    let delta = unit_to_delta(unit, n)?;
    Some(start_of_day(today - delta))
}

fn try_parse_last(s: &str, today: NaiveDate) -> Option<DateTime<Utc>> {
    let unit = s.strip_prefix("last ")?.trim_end_matches('s');
    let delta = unit_to_delta(unit, 1)?;
    Some(start_of_day(today - delta))
}

struct ParsedDate {
    dt: DateTime<Utc>,
    date_only: bool,
}

fn parse_human_date(s: &str) -> Result<ParsedDate, String> {
    let trimmed = s.trim();
    let lower = trimmed.to_lowercase();
    let today = Utc::now().date_naive();

    if lower == "today" || lower == "now" {
        return Ok(ParsedDate {
            dt: start_of_day(today),
            date_only: true,
        });
    }
    if lower == "yesterday" {
        return Ok(ParsedDate {
            dt: start_of_day(today - TimeDelta::days(1)),
            date_only: true,
        });
    }
    if let Some(dt) = try_parse_ago(&lower, today) {
        return Ok(ParsedDate {
            dt,
            date_only: true,
        });
    }
    if let Some(dt) = try_parse_last(&lower, today) {
        return Ok(ParsedDate {
            dt,
            date_only: true,
        });
    }
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Ok(ParsedDate {
            dt: start_of_day(date),
            date_only: true,
        });
    }
    if let Some(dt) = parse_timestamp(trimmed) {
        return Ok(ParsedDate {
            dt,
            date_only: false,
        });
    }

    Err(format!(
        "Cannot parse date: '{trimmed}'. Try: today, yesterday, 3 days ago, last week, or YYYY-MM-DD"
    ))
}

fn build_date_range(
    since: Option<&str>,
    until: Option<&str>,
    date: Option<&str>,
) -> Result<DateRange, String> {
    if let Some(d) = date {
        let parsed = parse_human_date(d)?;
        let end = parsed.dt + TimeDelta::days(1);
        return Ok(DateRange {
            since: Some(parsed.dt),
            until: Some(end),
        });
    }

    let since_dt = since
        .map(|s| parse_human_date(s).map(|p| p.dt))
        .transpose()?;
    let until_dt = until
        .map(|u| {
            let parsed = parse_human_date(u)?;
            let boundary = if parsed.date_only {
                parsed.dt + TimeDelta::days(1)
            } else {
                parsed.dt
            };
            Ok::<_, String>(boundary)
        })
        .transpose()?;

    Ok(DateRange {
        since: since_dt,
        until: until_dt,
    })
}

// ─── Output Formatting ─────────────────────────────────────────────

#[derive(Serialize)]
struct SearchOutput {
    search_x_agents: SearchInner,
}

#[derive(Serialize)]
struct SearchInner {
    query: String,
    total_matches: usize,
    top_results: Vec<ResultEntry>,
}

#[derive(Serialize)]
struct ResultEntry {
    id: usize,
    source: String,
    project: String,
    date: String,
    session: String,
    snippet: String,
}

fn format_project_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

fn print_results(matches: &[DeepMatch], query: &str, limit: usize) {
    let total = matches.len();
    let displayed = &matches[..total.min(limit)];

    let results: Vec<ResultEntry> = displayed
        .iter()
        .enumerate()
        .map(|(i, m)| ResultEntry {
            id: i + 1,
            source: m.source.clone(),
            project: format_project_path(&m.project_path),
            date: m.timestamp.clone(),
            session: m.session_id.clone(),
            snippet: m.snippet.clone(),
        })
        .collect();

    let output = SearchOutput {
        search_x_agents: SearchInner {
            query: query.to_string(),
            total_matches: total,
            top_results: results,
        },
    };

    // Unwrap safe: serialization of simple strings/integers cannot fail
    println!("{}", serde_yaml::to_string(&output).unwrap());
}

// ─── Dispatch ───────────────────────────────────────────────────────

fn build_sources(cli: &Cli) -> Vec<Box<dyn SessionSource + '_>> {
    let mut sources: Vec<Box<dyn SessionSource>> = Vec::new();

    let any_flag = cli.claude
        || cli.openclaw
        || cli.codex
        || cli.pi
        || cli.antigravity
        || cli.cowork
        || cli.hermes;
    let all = !any_flag;

    if all || cli.claude {
        let s = ClaudeSource;
        if !s.session_roots().is_empty() {
            sources.push(Box::new(s));
        }
    }
    if all || cli.openclaw {
        let s = OpenClawSource::new(cli.agent.clone());
        if !s.session_roots().is_empty() {
            sources.push(Box::new(s));
        }
    }
    if all || cli.codex {
        let s = CodexSource;
        if !s.session_roots().is_empty() {
            sources.push(Box::new(s));
        }
    }
    if all || cli.pi {
        let s = PiSource;
        if !s.session_roots().is_empty() {
            sources.push(Box::new(s));
        }
    }
    if all || cli.antigravity {
        let s = AntigravitySource;
        if !s.session_roots().is_empty() {
            sources.push(Box::new(s));
        }
    }
    if all || cli.cowork {
        let s = CoworkSource;
        if !s.session_roots().is_empty() {
            sources.push(Box::new(s));
        }
    }
    if all || cli.hermes {
        let s = HermesSource;
        if !s.session_roots().is_empty() {
            sources.push(Box::new(s));
        }
    }

    sources
}

// ─── Main ───────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    let query = cli.query.join(" ");
    if query.is_empty() {
        eprintln!("ERROR: No search query provided");
        std::process::exit(1);
    }

    let date_range = match build_date_range(
        cli.since.as_deref(),
        cli.until.as_deref(),
        cli.date.as_deref(),
    ) {
        Ok(dr) => dr,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    };

    let sources = build_sources(&cli);

    if sources.is_empty() {
        eprintln!("ERROR: No agent session directories found.");
        eprintln!("       Install at least one agent (Claude Code, Codex, Pi, OpenClaw, etc.)");
        std::process::exit(1);
    }

    let project_filter = cli.project.as_deref();
    let matches = search_all_sources(&query, &sources, project_filter, &date_range);
    print_results(&matches, &query, cli.limit);
}
