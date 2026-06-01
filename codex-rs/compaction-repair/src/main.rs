use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use chrono::Local;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use sqlx::AssertSqlSafe;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use tokio::time::sleep;

const STATE_DB_FILENAME: &str = "state_5.sqlite";
const LOGS_DB_FILENAME: &str = "logs_2.sqlite";
const SESSIONS_DIR: &str = "sessions";
const ARCHIVED_SESSIONS_DIR: &str = "archived_sessions";
const BACKUP_DIR: &str = "session-slim-backups";
const DEFAULT_KEEP_VISIBLE_MESSAGES: usize = 500;
const DEFAULT_TARGET_BYTES: usize = 450_000;
const IMAGE_REPLACEMENT: &str = "[Compaction repair note: embedded image payload removed from active rollout; original preserved in backup.]";

#[derive(Debug, Parser)]
#[command(name = "codex-compaction-repair")]
#[command(about = "Inspect and repair local Codex sessions wedged by compaction errors.")]
struct Cli {
    #[arg(long, value_name = "PATH")]
    codex_home: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Inspect(InspectArgs),
    Repair(RepairArgs),
}

#[derive(Debug, Args)]
struct InspectArgs {
    #[command(flatten)]
    target: TargetArgs,

    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct RepairArgs {
    #[command(flatten)]
    target: TargetArgs,

    /// Actually rewrite the rollout and reset token metadata. Omit for dry run.
    #[arg(long)]
    apply: bool,

    /// Repair profile from least to most destructive: gentle, standard, aggressive, emergency.
    #[arg(long, value_enum, default_value = "gentle")]
    profile: RepairProfile,

    /// Repair strategy: manual uses one profile; auto steps profiles down until target bytes is met.
    #[arg(long, value_enum, default_value = "manual")]
    strategy: RepairStrategy,

    /// Auto strategy target repaired rollout size in bytes.
    #[arg(long, default_value_t = DEFAULT_TARGET_BYTES)]
    target_bytes: usize,

    /// Keep every visible user/assistant message instead of only the recent tail.
    #[arg(long)]
    keep_all_visible_messages: bool,

    /// Override the selected profile's visible message retention.
    #[arg(long)]
    keep_visible_messages: Option<usize>,

    /// Override the selected profile's retained turn_context count.
    #[arg(long)]
    keep_turn_context: Option<usize>,

    /// Override the selected profile's retained event marker count.
    #[arg(long)]
    keep_event_markers: Option<usize>,

    /// Seconds the rollout file must remain unchanged before an applied repair.
    #[arg(long, default_value_t = 10)]
    stability_window_seconds: u64,

    /// Skip the rollout stability check.
    #[arg(long)]
    force: bool,

    /// Do not reset threads.tokens_used in state_5.sqlite.
    #[arg(long)]
    skip_token_reset: bool,

    /// Do not write a local Markdown handoff into the backup directory.
    #[arg(long)]
    skip_handoff: bool,

    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct TargetArgs {
    #[arg(long, value_name = "THREAD_ID")]
    thread_id: String,

    #[arg(long, value_name = "PATH")]
    rollout_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct InspectReport {
    codex_home: PathBuf,
    thread_id: String,
    rollout_path: Option<PathBuf>,
    rollout_candidates: Vec<PathBuf>,
    thread_metadata: Option<ThreadMetadata>,
    log_summary: Option<LogSummary>,
    rollout_summary: Option<RolloutSummary>,
}

#[derive(Debug, Serialize)]
struct RepairReport {
    codex_home: PathBuf,
    thread_id: String,
    rollout_path: PathBuf,
    dry_run: bool,
    backup_dir: Option<PathBuf>,
    handoff_path: Option<PathBuf>,
    token_reset: Option<TokenResetReport>,
    profile: SlimOptions,
    strategy: RepairStrategy,
    auto_attempts: Option<Vec<AutoAttempt>>,
    slim: SlimReport,
}

#[derive(Debug, Serialize)]
struct ThreadMetadata {
    title: Option<String>,
    tokens_used: Option<i64>,
    model: Option<String>,
    cli_version: Option<String>,
    rollout_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenResetReport {
    old_tokens_used: Option<i64>,
    rows_updated: u64,
}

#[derive(Debug, Serialize)]
struct LogSummary {
    compact_related_logs: i64,
    recent_compact_related_logs: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RolloutSummary {
    bytes: u64,
    lines: usize,
    invalid_json_lines: usize,
    data_image_occurrences: usize,
    compact_error_like_lines: usize,
    item_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct SlimReport {
    original_bytes: u64,
    repaired_bytes: usize,
    original_lines: usize,
    kept_lines: usize,
    dropped_lines: usize,
    invalid_json_lines: usize,
    data_image_replacements: usize,
    compact_error_like_lines_dropped: usize,
    item_counts: BTreeMap<String, usize>,
    kept_by_reason: BTreeMap<String, usize>,
    dropped_by_reason: BTreeMap<String, usize>,
}

#[derive(Debug)]
struct ParsedLine {
    value: Value,
    kind: LineKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum LineKind {
    SessionMeta,
    VisibleMessage,
    TurnContext,
    EventMarker,
    Drop,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RepairProfile {
    Gentle,
    Standard,
    Aggressive,
    Emergency,
}

impl RepairProfile {
    fn ordered_from(self) -> &'static [Self] {
        match self {
            Self::Gentle => &[
                Self::Gentle,
                Self::Standard,
                Self::Aggressive,
                Self::Emergency,
            ],
            Self::Standard => &[Self::Standard, Self::Aggressive, Self::Emergency],
            Self::Aggressive => &[Self::Aggressive, Self::Emergency],
            Self::Emergency => &[Self::Emergency],
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
enum RepairStrategy {
    Manual,
    Auto,
}

#[derive(Debug, Serialize)]
struct AutoAttempt {
    profile: &'static str,
    repaired_bytes: usize,
    kept_lines: usize,
    dropped_lines: usize,
    target_met: bool,
}

#[derive(Debug, Serialize)]
struct SlimOptions {
    keep_all_visible_messages: bool,
    keep_visible_messages: usize,
    keep_turn_context: usize,
    keep_event_markers: usize,
    profile: &'static str,
}

impl SlimOptions {
    fn from_args(args: &RepairArgs) -> Self {
        Self::from_profile(args.profile, args)
    }

    fn from_profile(profile: RepairProfile, args: &RepairArgs) -> Self {
        let mut options = match profile {
            RepairProfile::Gentle => Self {
                keep_all_visible_messages: true,
                keep_visible_messages: 0,
                keep_turn_context: 40,
                keep_event_markers: 160,
                profile: "gentle",
            },
            RepairProfile::Standard => Self {
                keep_all_visible_messages: false,
                keep_visible_messages: DEFAULT_KEEP_VISIBLE_MESSAGES,
                keep_turn_context: 20,
                keep_event_markers: 80,
                profile: "standard",
            },
            RepairProfile::Aggressive => Self {
                keep_all_visible_messages: false,
                keep_visible_messages: 180,
                keep_turn_context: 10,
                keep_event_markers: 40,
                profile: "aggressive",
            },
            RepairProfile::Emergency => Self {
                keep_all_visible_messages: false,
                keep_visible_messages: 60,
                keep_turn_context: 4,
                keep_event_markers: 12,
                profile: "emergency",
            },
        };

        if args.keep_all_visible_messages {
            options.keep_all_visible_messages = true;
        }
        if let Some(keep_visible_messages) = args.keep_visible_messages {
            options.keep_visible_messages = keep_visible_messages;
            options.keep_all_visible_messages = false;
        }
        if let Some(keep_turn_context) = args.keep_turn_context {
            options.keep_turn_context = keep_turn_context;
        }
        if let Some(keep_event_markers) = args.keep_event_markers {
            options.keep_event_markers = keep_event_markers;
        }
        options
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let codex_home = resolve_codex_home(cli.codex_home)?;

    match cli.command {
        Command::Inspect(args) => {
            let report = inspect(codex_home, &args.target).await?;
            print_report(&report, args.json)?;
        }
        Command::Repair(args) => {
            let json_output = args.json;
            let report = repair(codex_home, args).await?;
            print_report(&report, json_output)?;
        }
    }

    Ok(())
}

fn resolve_codex_home(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize --codex-home {}", path.display()))?);
    }

    Ok(codex_utils_home_dir::find_codex_home()
        .context("failed to resolve Codex home")?
        .into_path_buf())
}

async fn inspect(codex_home: PathBuf, target: &TargetArgs) -> Result<InspectReport> {
    let state_path = codex_home.join(STATE_DB_FILENAME);
    let metadata = read_thread_metadata(state_path.as_path(), &target.thread_id).await?;
    let candidates = find_rollout_candidates(&codex_home, target).await?;
    let rollout_path = select_rollout_path(target, metadata.as_ref(), &candidates)?;
    let rollout_summary = rollout_path
        .as_ref()
        .map(|path| summarize_rollout(path.as_path()))
        .transpose()?;
    let logs_path = codex_home.join(LOGS_DB_FILENAME);
    let log_summary = summarize_compact_logs(logs_path.as_path(), &target.thread_id).await?;

    Ok(InspectReport {
        codex_home,
        thread_id: target.thread_id.clone(),
        rollout_path,
        rollout_candidates: candidates,
        thread_metadata: metadata,
        log_summary,
        rollout_summary,
    })
}

async fn repair(codex_home: PathBuf, args: RepairArgs) -> Result<RepairReport> {
    let state_path = codex_home.join(STATE_DB_FILENAME);
    let metadata = read_thread_metadata(state_path.as_path(), &args.target.thread_id).await?;
    let candidates = find_rollout_candidates(&codex_home, &args.target).await?;
    let rollout_path = select_rollout_path(&args.target, metadata.as_ref(), &candidates)?
        .context("no rollout path found for thread")?;

    if args.apply && !args.force && args.stability_window_seconds > 0 {
        wait_for_stable_file(
            rollout_path.as_path(),
            Duration::from_secs(args.stability_window_seconds),
        )
        .await?;
    }

    let (repaired_lines, slim, options, auto_attempts) = match args.strategy {
        RepairStrategy::Manual => {
            let options = SlimOptions::from_args(&args);
            let (repaired_lines, slim) = slim_rollout(rollout_path.as_path(), &options)?;
            (repaired_lines, slim, options, None)
        }
        RepairStrategy::Auto => {
            let (repaired_lines, slim, options, attempts) =
                choose_auto_profile(rollout_path.as_path(), &args)?;
            (repaired_lines, slim, options, Some(attempts))
        }
    };

    if !args.apply {
        return Ok(RepairReport {
            codex_home,
            thread_id: args.target.thread_id,
            rollout_path,
            dry_run: true,
            backup_dir: None,
            handoff_path: None,
            token_reset: None,
            profile: options,
            strategy: args.strategy,
            auto_attempts,
            slim,
        });
    }

    let backup_dir = create_backup_dir(&codex_home, &args.target.thread_id)?;
    fs::copy(&rollout_path, backup_dir.join(file_name(&rollout_path)?))
        .with_context(|| format!("failed to back up rollout {}", rollout_path.display()))?;
    backup_sqlite_files(
        state_path.as_path(),
        backup_dir.as_path(),
        STATE_DB_FILENAME,
    )
    .await?;
    backup_sqlite_files(
        codex_home.join(LOGS_DB_FILENAME).as_path(),
        backup_dir.as_path(),
        LOGS_DB_FILENAME,
    )
    .await?;

    let handoff_path = if args.skip_handoff {
        None
    } else {
        Some(write_handoff(
            backup_dir.as_path(),
            &args.target.thread_id,
            rollout_path.as_path(),
            repaired_lines.as_slice(),
        )?)
    };

    write_repaired_rollout(rollout_path.as_path(), repaired_lines.as_slice())?;

    let token_reset = if args.skip_token_reset {
        None
    } else {
        Some(reset_thread_tokens(state_path.as_path(), &args.target.thread_id).await?)
    };

    let report = RepairReport {
        codex_home,
        thread_id: args.target.thread_id,
        rollout_path,
        dry_run: false,
        backup_dir: Some(backup_dir.clone()),
        handoff_path,
        token_reset,
        profile: options,
        strategy: args.strategy,
        auto_attempts,
        slim,
    };

    fs::write(
        backup_dir.join("repair-report.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )
    .context("failed to write repair report")?;

    Ok(report)
}

fn print_report<T: Serialize>(report: &T, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}

async fn read_thread_metadata(path: &Path, thread_id: &str) -> Result<Option<ThreadMetadata>> {
    if !path.exists() {
        return Ok(None);
    }

    let pool = open_sqlite_pool(path, true).await?;
    let row = sqlx::query(
        "SELECT title, tokens_used, model, cli_version, rollout_path FROM threads WHERE id = ?",
    )
    .bind(thread_id)
    .fetch_optional(&pool)
    .await
    .with_context(|| format!("failed to read thread metadata from {}", path.display()))?;

    Ok(row.map(|row| ThreadMetadata {
        title: row.try_get("title").ok(),
        tokens_used: row.try_get("tokens_used").ok(),
        model: row.try_get("model").ok(),
        cli_version: row.try_get("cli_version").ok(),
        rollout_path: row.try_get("rollout_path").ok(),
    }))
}

async fn summarize_compact_logs(path: &Path, thread_id: &str) -> Result<Option<LogSummary>> {
    if !path.exists() {
        return Ok(None);
    }

    let pool = open_sqlite_pool(path, true).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM logs
         WHERE thread_id = ?
           AND (
             feedback_log_body LIKE '%compact%'
             OR feedback_log_body LIKE '%Failed to run pre-sampling%'
             OR feedback_log_body LIKE '%/responses/compact%'
           )",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(0);

    let rows = sqlx::query(
        "SELECT feedback_log_body FROM logs
         WHERE thread_id = ?
           AND (
             feedback_log_body LIKE '%compact%'
             OR feedback_log_body LIKE '%Failed to run pre-sampling%'
             OR feedback_log_body LIKE '%/responses/compact%'
           )
         ORDER BY id DESC
         LIMIT 5",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let recent_compact_related_logs = rows
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("feedback_log_body").ok())
        .map(|body| normalize_whitespace(&body))
        .map(|body| redact_sensitive(&body))
        .map(|body| truncate(&body, 700))
        .collect();

    Ok(Some(LogSummary {
        compact_related_logs: count,
        recent_compact_related_logs,
    }))
}

async fn open_sqlite_pool(path: &Path, read_only: bool) -> Result<sqlx::SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(read_only);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open sqlite db {}", path.display()))
}

async fn find_rollout_candidates(codex_home: &Path, target: &TargetArgs) -> Result<Vec<PathBuf>> {
    if let Some(path) = &target.rollout_path {
        return Ok(vec![path.canonicalize().with_context(|| {
            format!("failed to canonicalize rollout path {}", path.display())
        })?]);
    }

    let mut candidates = Vec::new();
    for dir in [SESSIONS_DIR, ARCHIVED_SESSIONS_DIR] {
        let root = codex_home.join(dir);
        if root.exists() {
            find_matching_rollouts(&root, &target.thread_id, &mut candidates)?;
        }
    }

    candidates.sort_by(|a, b| modified_time(b).cmp(&modified_time(a)));
    Ok(candidates)
}

fn select_rollout_path(
    target: &TargetArgs,
    metadata: Option<&ThreadMetadata>,
    candidates: &[PathBuf],
) -> Result<Option<PathBuf>> {
    if let Some(path) = &target.rollout_path {
        return Ok(Some(path.clone()));
    }

    if let Some(path) = metadata
        .and_then(|m| m.rollout_path.as_deref())
        .map(PathBuf::from)
        .filter(|path| path.exists())
    {
        return Ok(Some(path.canonicalize().unwrap_or(path)));
    }

    if candidates.len() > 1 {
        eprintln!(
            "warning: found {} rollout candidates for {}; using newest modified file",
            candidates.len(),
            target.thread_id
        );
    }

    Ok(candidates.first().cloned())
}

fn find_matching_rollouts(root: &Path, thread_id: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("warning: failed to read {}: {err}", dir.display());
                continue;
            }
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(thread_id) && name.ends_with(".jsonl"))
            {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn summarize_rollout(path: &Path) -> Result<RolloutSummary> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat rollout {}", path.display()))?;
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read rollout {}", path.display()))?;
    let mut item_counts = BTreeMap::new();
    let mut invalid_json_lines = 0;
    let mut data_image_occurrences = 0;
    let mut compact_error_like_lines = 0;
    let mut lines = 0;

    for line in content.lines() {
        lines += 1;
        if line.contains("data:image/") {
            data_image_occurrences += line.matches("data:image/").count();
        }
        if is_compact_error_text(line) {
            compact_error_like_lines += 1;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                *item_counts.entry(type_label(&value)).or_insert(0) += 1;
            }
            Err(_) => invalid_json_lines += 1,
        }
    }

    Ok(RolloutSummary {
        bytes: metadata.len(),
        lines,
        invalid_json_lines,
        data_image_occurrences,
        compact_error_like_lines,
        item_counts,
    })
}

fn slim_rollout(path: &Path, options: &SlimOptions) -> Result<(Vec<String>, SlimReport)> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat rollout {}", path.display()))?;
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read rollout {}", path.display()))?;
    slim_rollout_content(&content, metadata.len(), options)
}

fn choose_auto_profile(
    path: &Path,
    args: &RepairArgs,
) -> Result<(Vec<String>, SlimReport, SlimOptions, Vec<AutoAttempt>)> {
    let mut attempts = Vec::new();
    let mut best = None;
    for profile in args.profile.ordered_from() {
        let options = SlimOptions::from_profile(*profile, args);
        let (repaired_lines, slim) = slim_rollout(path, &options)?;
        let target_met = slim.repaired_bytes <= args.target_bytes;
        attempts.push(AutoAttempt {
            profile: options.profile,
            repaired_bytes: slim.repaired_bytes,
            kept_lines: slim.kept_lines,
            dropped_lines: slim.dropped_lines,
            target_met,
        });
        best = Some((repaired_lines, slim, options));
        if target_met {
            break;
        }
    }

    best.map(|(repaired_lines, slim, options)| (repaired_lines, slim, options, attempts))
        .context("auto strategy did not evaluate any profiles")
}

fn slim_rollout_content(
    content: &str,
    original_bytes: u64,
    options: &SlimOptions,
) -> Result<(Vec<String>, SlimReport)> {
    let mut parsed = Vec::new();
    let mut invalid_json_lines = 0;
    let mut item_counts = BTreeMap::new();
    let mut visible_indexes = Vec::new();
    let mut turn_context_indexes = Vec::new();
    let mut marker_indexes = Vec::new();
    let mut session_meta_indexes = Vec::new();

    for line in content.lines() {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => {
                invalid_json_lines += 1;
                continue;
            }
        };

        *item_counts.entry(type_label(&value)).or_insert(0) += 1;
        let kind = classify_line(&value);
        let parsed_index = parsed.len();
        match kind {
            LineKind::SessionMeta => session_meta_indexes.push(parsed_index),
            LineKind::VisibleMessage => visible_indexes.push(parsed_index),
            LineKind::TurnContext => turn_context_indexes.push(parsed_index),
            LineKind::EventMarker => marker_indexes.push(parsed_index),
            LineKind::Drop => {}
        }

        parsed.push(ParsedLine { value, kind });
    }

    let mut keep = BTreeSet::new();
    if let Some(first) = session_meta_indexes.first() {
        keep.insert(*first);
    }
    if let Some(last) = session_meta_indexes.last() {
        keep.insert(*last);
    }

    if options.keep_all_visible_messages {
        keep.extend(visible_indexes.iter().copied());
    } else if options.keep_visible_messages > 0 {
        let start = visible_indexes
            .len()
            .saturating_sub(options.keep_visible_messages);
        keep.extend(visible_indexes[start..].iter().copied());
    }
    keep.extend(tail_indexes(
        &turn_context_indexes,
        options.keep_turn_context,
    ));
    keep.extend(tail_indexes(&marker_indexes, options.keep_event_markers));

    let mut kept_by_reason = BTreeMap::new();
    let mut dropped_by_reason = BTreeMap::new();
    let mut repaired_lines = Vec::new();
    let mut data_image_replacements = 0;
    let mut compact_error_like_lines_dropped = 0;

    for (idx, mut line) in parsed.into_iter().enumerate() {
        if keep.contains(&idx) {
            let reason = keep_reason(line.kind);
            *kept_by_reason.entry(reason.to_string()).or_insert(0) += 1;
            data_image_replacements += scrub_data_images(&mut line.value);
            repaired_lines.push(serde_json::to_string(&line.value)?);
        } else {
            let reason = drop_reason(&line.value, line.kind);
            if reason == "compact_error" {
                compact_error_like_lines_dropped += 1;
            }
            *dropped_by_reason.entry(reason.to_string()).or_insert(0) += 1;
        }
    }

    let repaired_bytes = repaired_lines.iter().map(|line| line.len() + 1).sum();
    let original_lines = content.lines().count();
    let kept_lines = repaired_lines.len();
    let dropped_lines = original_lines.saturating_sub(kept_lines);

    Ok((
        repaired_lines,
        SlimReport {
            original_bytes,
            repaired_bytes,
            original_lines,
            kept_lines,
            dropped_lines,
            invalid_json_lines,
            data_image_replacements,
            compact_error_like_lines_dropped,
            item_counts,
            kept_by_reason,
            dropped_by_reason,
        },
    ))
}

fn tail_indexes(indexes: &[usize], count: usize) -> impl Iterator<Item = usize> + '_ {
    indexes[indexes.len().saturating_sub(count)..]
        .iter()
        .copied()
}

fn classify_line(value: &Value) -> LineKind {
    let top_type = string_field(value, "type").unwrap_or_default();
    if top_type == "session_meta" {
        return LineKind::SessionMeta;
    }
    if top_type == "turn_context" {
        return LineKind::TurnContext;
    }
    if top_type == "compacted" {
        return LineKind::Drop;
    }

    let payload = value.get("payload").unwrap_or(value);
    let payload_type = string_field(payload, "type").unwrap_or_default();

    if top_type == "response_item" && payload_type == "message" {
        let role = string_field(payload, "role").unwrap_or_default();
        if matches!(role, "user" | "assistant") && !is_compact_error_value(value) {
            return LineKind::VisibleMessage;
        }
    }

    if top_type == "event_msg" {
        if matches!(
            payload_type,
            "user_message"
                | "agent_message"
                | "task_started"
                | "task_complete"
                | "thread_name_updated"
        ) && !is_compact_error_value(value)
        {
            return if matches!(payload_type, "user_message" | "agent_message") {
                LineKind::VisibleMessage
            } else {
                LineKind::EventMarker
            };
        }
    }

    LineKind::Drop
}

fn keep_reason(kind: LineKind) -> &'static str {
    match kind {
        LineKind::SessionMeta => "session_meta",
        LineKind::VisibleMessage => "visible_message",
        LineKind::TurnContext => "turn_context",
        LineKind::EventMarker => "event_marker",
        LineKind::Drop => "unknown",
    }
}

fn drop_reason(value: &Value, kind: LineKind) -> &'static str {
    if is_compact_error_value(value) {
        return "compact_error";
    }

    let label = type_label(value);
    if label == "compacted" {
        return "compacted";
    }
    if label.contains("reasoning") {
        return "reasoning";
    }
    if label.contains("function")
        || label.contains("tool")
        || label.contains("shell")
        || label.contains("web_search")
        || label.contains("image_generation")
    {
        return "tool_or_backend";
    }
    if kind == LineKind::VisibleMessage {
        return "old_visible_message";
    }
    "backend"
}

fn type_label(value: &Value) -> String {
    let top = string_field(value, "type").unwrap_or("unknown");
    let payload = value.get("payload").unwrap_or(value);
    let payload_type = string_field(payload, "type");
    match payload_type {
        Some(payload_type) if payload_type != top => format!("{top}/{payload_type}"),
        _ => top.to_string(),
    }
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn is_compact_error_value(value: &Value) -> bool {
    value_text_contains(value, is_compact_error_text)
}

fn is_compact_error_text(text: &str) -> bool {
    text.contains("Error running remote compact task")
        || text.contains("Failed to run pre-sampling compact")
        || text.contains("remote compaction failed")
        || text.contains("/responses/compact")
        || text.contains("/backend-api/codex/responses/compact")
}

fn value_text_contains(value: &Value, predicate: fn(&str) -> bool) -> bool {
    match value {
        Value::String(text) => predicate(text),
        Value::Array(items) => items
            .iter()
            .any(|item| value_text_contains(item, predicate)),
        Value::Object(map) => map
            .values()
            .any(|item| value_text_contains(item, predicate)),
        _ => false,
    }
}

fn scrub_data_images(value: &mut Value) -> usize {
    match value {
        Value::String(text) => {
            if text.starts_with("data:image/") {
                *text = IMAGE_REPLACEMENT.to_string();
                1
            } else {
                0
            }
        }
        Value::Array(items) => items.iter_mut().map(scrub_data_images).sum(),
        Value::Object(map) => scrub_data_images_in_object(map),
        _ => 0,
    }
}

fn scrub_data_images_in_object(map: &mut Map<String, Value>) -> usize {
    let mut count = 0;
    for value in map.values_mut() {
        count += scrub_data_images(value);
    }
    count
}

async fn wait_for_stable_file(path: &Path, duration: Duration) -> Result<()> {
    let first = file_signature(path)?;
    sleep(duration).await;
    let second = file_signature(path)?;
    if first != second {
        bail!(
            "rollout changed during stability check; wait for the active turn to finish or pass --force"
        );
    }
    Ok(())
}

fn file_signature(path: &Path) -> Result<(u64, Option<SystemTime>)> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat rollout {}", path.display()))?;
    Ok((metadata.len(), metadata.modified().ok()))
}

fn create_backup_dir(codex_home: &Path, thread_id: &str) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let backup_dir = codex_home
        .join(BACKUP_DIR)
        .join(format!("{timestamp}-{thread_id}"));
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("failed to create backup dir {}", backup_dir.display()))?;
    Ok(backup_dir)
}

async fn backup_sqlite_files(source: &Path, backup_dir: &Path, name: &str) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }

    let backup_path = backup_dir.join(name);
    match vacuum_into(source, backup_path.as_path()).await {
        Ok(()) => Ok(()),
        Err(err) => {
            eprintln!(
                "warning: SQLite VACUUM INTO failed for {}; falling back to file copy: {err}",
                source.display()
            );
            fs::copy(source, &backup_path).with_context(|| {
                format!(
                    "failed to copy sqlite db {} to {}",
                    source.display(),
                    backup_path.display()
                )
            })?;
            for suffix in ["-wal", "-shm"] {
                let sidecar = PathBuf::from(format!("{}{}", source.display(), suffix));
                if sidecar.exists() {
                    fs::copy(&sidecar, backup_dir.join(format!("{name}{suffix}")))?;
                }
            }
            Ok(())
        }
    }
}

async fn vacuum_into(source: &Path, dest: &Path) -> Result<()> {
    let pool = open_sqlite_pool(source, true).await?;
    let escaped = dest.to_string_lossy().replace('\'', "''");
    let query = format!("VACUUM main INTO '{escaped}'");
    // SQLite cannot bind the VACUUM INTO destination, so the path is single-quote
    // escaped above before marking this statement as audited.
    sqlx::query(AssertSqlSafe(query))
        .execute(&pool)
        .await
        .with_context(|| {
            format!(
                "failed to VACUUM {} into {}",
                source.display(),
                dest.display()
            )
        })?;
    Ok(())
}

fn write_handoff(
    backup_dir: &Path,
    thread_id: &str,
    rollout_path: &Path,
    repaired_lines: &[String],
) -> Result<PathBuf> {
    let path = backup_dir.join("handoff.md");
    let mut file = fs::File::create(&path)
        .with_context(|| format!("failed to create handoff {}", path.display()))?;
    writeln!(file, "# Compaction Repair Handoff")?;
    writeln!(file)?;
    writeln!(file, "- Thread ID: `{thread_id}`")?;
    writeln!(file, "- Repaired rollout: `{}`", rollout_path.display())?;
    writeln!(
        file,
        "- Use this file as continuity context if the repaired thread still fails after restarting Codex."
    )?;
    writeln!(file)?;
    writeln!(file, "## Recent Visible Messages")?;

    let handoff_start = repaired_lines.len().saturating_sub(40);
    for line in &repaired_lines[handoff_start..] {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if classify_line(&value) != LineKind::VisibleMessage {
            continue;
        }
        let role = visible_role(&value).unwrap_or("message");
        let text = visible_text(&value);
        if text.is_empty() {
            continue;
        }
        writeln!(file)?;
        writeln!(file, "### {role}")?;
        writeln!(file)?;
        writeln!(file, "{}", truncate(&text, 3000))?;
    }

    Ok(path)
}

fn visible_role(value: &Value) -> Option<&str> {
    let payload = value.get("payload").unwrap_or(value);
    if let Some(role) = string_field(payload, "role") {
        return Some(role);
    }

    match string_field(payload, "type") {
        Some("user_message") => Some("user"),
        Some("agent_message") => Some("assistant"),
        _ => None,
    }
}

fn visible_text(value: &Value) -> String {
    let mut pieces = Vec::new();
    collect_text(value, &mut pieces);
    pieces
        .into_iter()
        .filter(|piece| !piece.starts_with("data:image/"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_text(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if !text.is_empty() && text.len() < 20_000 {
                out.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_text(item, out);
            }
        }
        Value::Object(map) => {
            for key in ["text", "message", "content"] {
                if let Some(value) = map.get(key) {
                    collect_text(value, out);
                }
            }
        }
        _ => {}
    }
}

fn write_repaired_rollout(path: &Path, lines: &[String]) -> Result<()> {
    let mut output = lines.join("\n");
    output.push('\n');
    fs::write(path, output)
        .with_context(|| format!("failed to write repaired rollout {}", path.display()))
}

async fn reset_thread_tokens(path: &Path, thread_id: &str) -> Result<TokenResetReport> {
    if !path.exists() {
        return Ok(TokenResetReport {
            old_tokens_used: None,
            rows_updated: 0,
        });
    }

    let pool = open_sqlite_pool(path, false).await?;
    let old_tokens_used = sqlx::query_scalar("SELECT tokens_used FROM threads WHERE id = ?")
        .bind(thread_id)
        .fetch_optional(&pool)
        .await
        .context("failed to read old tokens_used")?;
    let result = sqlx::query("UPDATE threads SET tokens_used = 0 WHERE id = ?")
        .bind(thread_id)
        .execute(&pool)
        .await
        .context("failed to reset tokens_used")?;

    Ok(TokenResetReport {
        old_tokens_used,
        rows_updated: result.rows_affected(),
    })
}

fn file_name(path: &Path) -> Result<&std::ffi::OsStr> {
    path.file_name()
        .with_context(|| format!("path has no file name: {}", path.display()))
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn normalize_whitespace(text: &str) -> String {
    let mut normalized = String::new();
    for part in text.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(part);
    }
    normalized
}

fn redact_sensitive(text: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for part in text.split_whitespace() {
        let cleaned = part.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';'));
        let lower = cleaned.to_ascii_lowercase();
        if redact_next {
            output.push("[redacted]".to_string());
            redact_next = false;
        } else if lower == "bearer" {
            output.push(part.to_string());
            redact_next = true;
        } else if cleaned.starts_with("sk-") && cleaned.len() > 16 {
            output.push(part.replace(cleaned, "sk-[redacted]"));
        } else if lower.contains("api_key") || lower.contains("api-key") {
            output.push("[api_key redacted]".to_string());
        } else {
            output.push(part.to_string());
        }
    }
    output.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(value: Value) -> String {
        serde_json::to_string(&value).unwrap()
    }

    #[test]
    fn slim_keeps_recent_visible_messages_and_drops_backend_fat() {
        let content = [
            line(json!({"type":"session_meta","payload":{"id":"first"}})),
            line(json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"old user"}]}})),
            line(json!({"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"big"}})),
            line(json!({"type":"response_item","payload":{"type":"function_call_output","output":"large output"}})),
            line(json!({"type":"response_item","payload":{"type":"reasoning","encrypted_content":"secret"}})),
            line(json!({"type":"compacted","payload":{"replacement_history":["huge"]}})),
            line(json!({"type":"event_msg","payload":{"type":"agent_message","message":"Error running remote compact task: /responses/compact"}})),
            line(json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"new assistant"}]}})),
            line(json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"new user"},{"type":"input_image","image_url":"data:image/png;base64,abc"}]}})),
            line(json!({"type":"session_meta","payload":{"id":"last"}})),
        ]
        .join("\n");

        let (lines, report) = slim_rollout_content(
            &content,
            content.len() as u64,
            &SlimOptions {
                keep_all_visible_messages: false,
                keep_visible_messages: 2,
                keep_turn_context: 20,
                keep_event_markers: 80,
                profile: "test",
            },
        )
        .unwrap();

        let repaired = lines.join("\n");
        assert!(repaired.contains("new assistant"));
        assert!(repaired.contains("new user"));
        assert!(repaired.contains("session_meta"));
        assert!(!repaired.contains("old user"));
        assert!(!repaired.contains("function_call"));
        assert!(!repaired.contains("encrypted_content"));
        assert!(!repaired.contains("/responses/compact"));
        assert!(!repaired.contains("data:image/png"));
        assert!(repaired.contains(IMAGE_REPLACEMENT));
        assert_eq!(report.data_image_replacements, 1);
        assert_eq!(report.compact_error_like_lines_dropped, 1);
    }

    #[test]
    fn slim_can_keep_all_visible_messages() {
        let content = [
            line(json!({"type":"session_meta","payload":{"id":"first"}})),
            line(json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"old user"}]}})),
            line(json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"assistant"}]}})),
        ]
        .join("\n");

        let (lines, _) = slim_rollout_content(
            &content,
            content.len() as u64,
            &SlimOptions {
                keep_all_visible_messages: true,
                keep_visible_messages: 1,
                keep_turn_context: 20,
                keep_event_markers: 80,
                profile: "test",
            },
        )
        .unwrap();

        let repaired = lines.join("\n");
        assert!(repaired.contains("old user"));
        assert!(repaired.contains("assistant"));
    }

    #[test]
    fn compact_error_detection_is_specific() {
        assert!(is_compact_error_text(
            "Failed to run pre-sampling compact: stream disconnected"
        ));
        assert!(!is_compact_error_text(
            "Can you help design a better compaction recovery tool?"
        ));
    }
}
