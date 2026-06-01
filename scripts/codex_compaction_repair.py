#!/usr/bin/env python3
"""Inspect and repair local Codex sessions wedged by compaction failures.

This is a standalone manual tool. It defaults to read-only behavior and only
changes local Codex state when `repair --apply` is passed.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shutil
import sqlite3
import sys
import time
from collections import Counter
from pathlib import Path
from typing import Any


STATE_DB = "state_5.sqlite"
LOGS_DB = "logs_2.sqlite"
BACKUP_DIR = "session-slim-backups"
IMAGE_REPLACEMENT = (
    "[Compaction repair note: embedded image payload removed from active "
    "rollout; original preserved in backup.]"
)
COMPACT_ERROR_PATTERNS = (
    "Error running remote compact task",
    "Failed to run pre-sampling compact",
    "remote compaction failed",
    "/responses/compact",
    "/backend-api/codex/responses/compact",
)
PROFILE_ORDER = ("gentle", "standard", "aggressive", "emergency")
DEFAULT_PROFILE = "gentle"
DEFAULT_TARGET_BYTES = 450_000
REPAIR_PROFILES: dict[str, dict[str, int | bool]] = {
    # Least destructive: keep all visible chat, remove only backend/tool fat.
    "gentle": {
        "keep_all_visible": True,
        "keep_visible_messages": 0,
        "keep_turn_context": 40,
        "keep_event_markers": 160,
    },
    # Default: this matches the repair pattern that has worked while preserving
    # a larger conversation tail for active coding sessions.
    "standard": {
        "keep_all_visible": False,
        "keep_visible_messages": 500,
        "keep_turn_context": 20,
        "keep_event_markers": 80,
    },
    # More aggressive: use when standard still leaves the compact request large.
    "aggressive": {
        "keep_all_visible": False,
        "keep_visible_messages": 180,
        "keep_turn_context": 10,
        "keep_event_markers": 40,
    },
    # Last resort for sessions that remain wedged after restart.
    "emergency": {
        "keep_all_visible": False,
        "keep_visible_messages": 60,
        "keep_turn_context": 4,
        "keep_event_markers": 12,
    },
}


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Inspect and repair local Codex compaction-wedged sessions."
    )
    parser.add_argument("--codex-home", type=Path, default=default_codex_home())
    sub = parser.add_subparsers(dest="command", required=True)

    inspect = sub.add_parser("inspect", help="Read-only session diagnostics")
    add_target_args(inspect)
    inspect.add_argument("--json", action="store_true")

    repair = sub.add_parser("repair", help="Dry-run or apply a local repair")
    add_target_args(repair)
    repair.add_argument("--apply", action="store_true")
    repair.add_argument(
        "--strategy",
        choices=("manual", "auto"),
        default="manual",
        help="manual uses one profile; auto tries profiles from gentle toward emergency until target bytes is met",
    )
    repair.add_argument(
        "--profile",
        choices=PROFILE_ORDER,
        default=DEFAULT_PROFILE,
        help="repair profile from least to most destructive: gentle, standard, aggressive, emergency",
    )
    repair.add_argument(
        "--target-bytes",
        type=int,
        default=DEFAULT_TARGET_BYTES,
        help="auto strategy target repaired rollout size in bytes",
    )
    repair.add_argument("--keep-all-visible-messages", action="store_true")
    repair.add_argument(
        "--keep-visible-messages",
        type=int,
        help="override the selected profile's visible message retention",
    )
    repair.add_argument(
        "--keep-turn-context",
        type=int,
        help="override the selected profile's retained turn_context count",
    )
    repair.add_argument(
        "--keep-event-markers",
        type=int,
        help="override the selected profile's retained event marker count",
    )
    repair.add_argument("--stability-window-seconds", type=int, default=10)
    repair.add_argument("--force", action="store_true")
    repair.add_argument("--skip-token-reset", action="store_true")
    repair.add_argument("--skip-handoff", action="store_true")
    repair.add_argument("--json", action="store_true")

    args = parser.parse_args()
    codex_home = args.codex_home.expanduser().resolve()

    if args.command == "inspect":
        report = inspect_session(codex_home, args)
    elif args.command == "repair":
        report = repair_session(codex_home, args)
    else:
        raise AssertionError(args.command)

    print(json.dumps(report, indent=2, default=str))
    return 0


def add_target_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--thread-id", required=True)
    parser.add_argument("--rollout-path", type=Path)


def default_codex_home() -> Path:
    env = os.environ.get("CODEX_HOME")
    if env:
        return Path(env)
    return Path.home() / ".codex"


def inspect_session(codex_home: Path, args: argparse.Namespace) -> dict[str, Any]:
    metadata = read_thread_metadata(codex_home / STATE_DB, args.thread_id)
    candidates = find_rollout_candidates(codex_home, args.thread_id, args.rollout_path)
    rollout_path = select_rollout(args.rollout_path, metadata, candidates)
    return {
        "codex_home": codex_home,
        "thread_id": args.thread_id,
        "rollout_path": rollout_path,
        "rollout_candidates": candidates,
        "thread_metadata": metadata,
        "log_summary": summarize_logs(codex_home / LOGS_DB, args.thread_id),
        "rollout_summary": summarize_rollout(rollout_path) if rollout_path else None,
    }


def repair_session(codex_home: Path, args: argparse.Namespace) -> dict[str, Any]:
    metadata = read_thread_metadata(codex_home / STATE_DB, args.thread_id)
    candidates = find_rollout_candidates(codex_home, args.thread_id, args.rollout_path)
    rollout_path = select_rollout(args.rollout_path, metadata, candidates)
    if not rollout_path:
        raise SystemExit(f"no rollout found for thread {args.thread_id}")

    if args.apply and not args.force and args.stability_window_seconds > 0:
        wait_for_stable_file(rollout_path, args.stability_window_seconds)

    if args.strategy == "auto":
        repaired_lines, slim, profile, attempts = choose_auto_profile(rollout_path, args)
    else:
        profile = resolve_profile(args)
        repaired_lines, slim = slim_rollout(
            rollout_path,
            profile=profile,
        )
        attempts = None

    report: dict[str, Any] = {
        "codex_home": codex_home,
        "thread_id": args.thread_id,
        "rollout_path": rollout_path,
        "dry_run": not args.apply,
        "backup_dir": None,
        "handoff_path": None,
        "token_reset": None,
        "profile": profile,
        "strategy": args.strategy,
        "auto_attempts": attempts,
        "slim": slim,
    }

    if not args.apply:
        return report

    backup_dir = create_backup_dir(codex_home, args.thread_id)
    shutil.copy2(rollout_path, backup_dir / rollout_path.name)
    backup_sqlite(codex_home / STATE_DB, backup_dir / STATE_DB)
    backup_sqlite(codex_home / LOGS_DB, backup_dir / LOGS_DB)

    handoff_path = None
    if not args.skip_handoff:
        handoff_path = write_handoff(backup_dir, args.thread_id, rollout_path, repaired_lines)

    rollout_path.write_text("\n".join(repaired_lines) + "\n", encoding="utf-8")

    token_reset = None
    if not args.skip_token_reset:
        token_reset = reset_thread_tokens(codex_home / STATE_DB, args.thread_id)

    report.update(
        {
            "backup_dir": backup_dir,
            "handoff_path": handoff_path,
            "token_reset": token_reset,
        }
    )
    (backup_dir / "repair-report.json").write_text(
        json.dumps(report, indent=2, default=str) + "\n", encoding="utf-8"
    )
    return report


def read_thread_metadata(path: Path, thread_id: str) -> dict[str, Any] | None:
    if not path.exists():
        return None
    with sqlite3.connect(path) as con:
        con.row_factory = sqlite3.Row
        row = con.execute(
            """
            SELECT title, tokens_used, model, cli_version, rollout_path
            FROM threads
            WHERE id = ?
            """,
            (thread_id,),
        ).fetchone()
    return dict(row) if row else None


def summarize_logs(path: Path, thread_id: str) -> dict[str, Any] | None:
    if not path.exists():
        return None
    query = """
        FROM logs
        WHERE thread_id = ?
          AND (
            feedback_log_body LIKE '%compact%'
            OR feedback_log_body LIKE '%Failed to run pre-sampling%'
            OR feedback_log_body LIKE '%/responses/compact%'
          )
    """
    with sqlite3.connect(path) as con:
        count = con.execute("SELECT COUNT(*) " + query, (thread_id,)).fetchone()[0]
        rows = con.execute(
            "SELECT feedback_log_body " + query + " ORDER BY id DESC LIMIT 5",
            (thread_id,),
        ).fetchall()
    return {
        "compact_related_logs": count,
        "recent_compact_related_logs": [
            redact_sensitive(" ".join((row[0] or "").split()))[:700] for row in rows
        ],
    }


def resolve_profile(args: argparse.Namespace) -> dict[str, Any]:
    return resolve_profile_name(args.profile, args)


def resolve_profile_name(name: str, args: argparse.Namespace) -> dict[str, Any]:
    profile = dict(REPAIR_PROFILES[name])
    profile["name"] = name
    if args.keep_all_visible_messages:
        profile["keep_all_visible"] = True
    if args.keep_visible_messages is not None:
        profile["keep_visible_messages"] = args.keep_visible_messages
        profile["keep_all_visible"] = False
    if args.keep_turn_context is not None:
        profile["keep_turn_context"] = args.keep_turn_context
    if args.keep_event_markers is not None:
        profile["keep_event_markers"] = args.keep_event_markers
    return profile


def choose_auto_profile(
    rollout_path: Path, args: argparse.Namespace
) -> tuple[list[str], dict[str, Any], dict[str, Any], list[dict[str, Any]]]:
    start = PROFILE_ORDER.index(args.profile)
    attempts: list[dict[str, Any]] = []
    best: tuple[list[str], dict[str, Any], dict[str, Any]] | None = None
    for name in PROFILE_ORDER[start:]:
        profile = resolve_profile_name(name, args)
        repaired_lines, slim = slim_rollout(rollout_path, profile=profile)
        attempt = {
            "profile": name,
            "repaired_bytes": slim["repaired_bytes"],
            "kept_lines": slim["kept_lines"],
            "dropped_lines": slim["dropped_lines"],
            "target_met": slim["repaired_bytes"] <= args.target_bytes,
        }
        attempts.append(attempt)
        best = (repaired_lines, slim, profile)
        if attempt["target_met"]:
            break
    assert best is not None
    return best[0], best[1], best[2], attempts


def find_rollout_candidates(
    codex_home: Path, thread_id: str, rollout_path: Path | None
) -> list[Path]:
    if rollout_path:
        return [rollout_path.expanduser().resolve()]
    candidates: list[Path] = []
    for subdir in ("sessions", "archived_sessions"):
        root = codex_home / subdir
        if not root.exists():
            continue
        for path in root.rglob("*.jsonl"):
            if thread_id in path.name:
                candidates.append(path.resolve())
    candidates.sort(key=lambda p: p.stat().st_mtime if p.exists() else 0, reverse=True)
    return candidates


def select_rollout(
    explicit: Path | None, metadata: dict[str, Any] | None, candidates: list[Path]
) -> Path | None:
    if explicit:
        return explicit.expanduser().resolve()
    if metadata and metadata.get("rollout_path"):
        path = Path(metadata["rollout_path"])
        if path.exists():
            return path.resolve()
    if len(candidates) > 1:
        print(
            f"warning: found {len(candidates)} rollout candidates; using newest modified file",
            file=sys.stderr,
        )
    return candidates[0] if candidates else None


def summarize_rollout(path: Path) -> dict[str, Any]:
    item_counts: Counter[str] = Counter()
    invalid_json_lines = 0
    data_image_occurrences = 0
    compact_error_like_lines = 0
    lines = 0
    for raw in path.read_text(encoding="utf-8").splitlines():
        lines += 1
        data_image_occurrences += raw.count("data:image/")
        if is_compact_error_text(raw):
            compact_error_like_lines += 1
        try:
            value = json.loads(raw)
        except json.JSONDecodeError:
            invalid_json_lines += 1
            continue
        item_counts[type_label(value)] += 1
    return {
        "bytes": path.stat().st_size,
        "lines": lines,
        "invalid_json_lines": invalid_json_lines,
        "data_image_occurrences": data_image_occurrences,
        "compact_error_like_lines": compact_error_like_lines,
        "item_counts": dict(item_counts),
    }


def slim_rollout(path: Path, *, profile: dict[str, Any]) -> tuple[list[str], dict[str, Any]]:
    content = path.read_text(encoding="utf-8")
    return slim_rollout_content(
        content,
        original_bytes=path.stat().st_size,
        keep_all_visible=bool(profile["keep_all_visible"]),
        keep_visible_messages=int(profile["keep_visible_messages"]),
        keep_turn_context=int(profile["keep_turn_context"]),
        keep_event_markers=int(profile["keep_event_markers"]),
    )


def slim_rollout_content(
    content: str,
    *,
    original_bytes: int,
    keep_all_visible: bool,
    keep_visible_messages: int,
    keep_turn_context: int = 20,
    keep_event_markers: int = 80,
) -> tuple[list[str], dict[str, Any]]:
    parsed: list[dict[str, Any]] = []
    item_counts: Counter[str] = Counter()
    invalid_json_lines = 0
    session_meta: list[int] = []
    visible: list[int] = []
    turn_context: list[int] = []
    markers: list[int] = []

    for line_number, raw in enumerate(content.splitlines(), 1):
        try:
            value = json.loads(raw)
        except json.JSONDecodeError:
            invalid_json_lines += 1
            continue
        kind = classify_line(value)
        idx = len(parsed)
        item_counts[type_label(value)] += 1
        if kind == "session_meta":
            session_meta.append(idx)
        elif kind == "visible_message":
            visible.append(idx)
        elif kind == "turn_context":
            turn_context.append(idx)
        elif kind == "event_marker":
            markers.append(idx)
        parsed.append({"line_number": line_number, "value": value, "kind": kind})

    keep: set[int] = set()
    if session_meta:
        keep.add(session_meta[0])
        keep.add(session_meta[-1])
    if keep_all_visible:
        keep.update(visible)
    elif keep_visible_messages > 0:
        keep.update(visible[-keep_visible_messages:])
    if keep_turn_context > 0:
        keep.update(turn_context[-keep_turn_context:])
    if keep_event_markers > 0:
        keep.update(markers[-keep_event_markers:])

    repaired_lines: list[str] = []
    kept_by_reason: Counter[str] = Counter()
    dropped_by_reason: Counter[str] = Counter()
    data_image_replacements = 0
    compact_error_like_lines_dropped = 0

    for idx, item in enumerate(parsed):
        value = item["value"]
        kind = item["kind"]
        if idx in keep:
            kept_by_reason[kind] += 1
            data_image_replacements += scrub_data_images(value)
            repaired_lines.append(json.dumps(value, separators=(",", ":")))
        else:
            reason = drop_reason(value, kind)
            if reason == "compact_error":
                compact_error_like_lines_dropped += 1
            dropped_by_reason[reason] += 1

    repaired_bytes = sum(len(line) + 1 for line in repaired_lines)
    original_lines = len(content.splitlines())
    return repaired_lines, {
        "original_bytes": original_bytes,
        "repaired_bytes": repaired_bytes,
        "original_lines": original_lines,
        "kept_lines": len(repaired_lines),
        "dropped_lines": original_lines - len(repaired_lines),
        "invalid_json_lines": invalid_json_lines,
        "data_image_replacements": data_image_replacements,
        "compact_error_like_lines_dropped": compact_error_like_lines_dropped,
        "item_counts": dict(item_counts),
        "kept_by_reason": dict(kept_by_reason),
        "dropped_by_reason": dict(dropped_by_reason),
    }


def classify_line(value: dict[str, Any]) -> str:
    top_type = value.get("type")
    payload = value.get("payload") if isinstance(value.get("payload"), dict) else value
    payload_type = payload.get("type")
    if top_type == "session_meta":
        return "session_meta"
    if top_type == "turn_context":
        return "turn_context"
    if top_type == "compacted":
        return "drop"
    if (
        top_type == "response_item"
        and payload_type == "message"
        and payload.get("role") in ("user", "assistant")
        and not is_compact_error_value(value)
    ):
        return "visible_message"
    if top_type == "event_msg" and payload_type in {
        "user_message",
        "agent_message",
        "task_started",
        "task_complete",
        "thread_name_updated",
    }:
        if is_compact_error_value(value):
            return "drop"
        return "visible_message" if payload_type in {"user_message", "agent_message"} else "event_marker"
    return "drop"


def drop_reason(value: dict[str, Any], kind: str) -> str:
    if is_compact_error_value(value):
        return "compact_error"
    label = type_label(value)
    if label == "compacted":
        return "compacted"
    if "reasoning" in label:
        return "reasoning"
    if any(part in label for part in ("function", "tool", "shell", "web_search", "image_generation")):
        return "tool_or_backend"
    if kind == "visible_message":
        return "old_visible_message"
    return "backend"


def type_label(value: dict[str, Any]) -> str:
    top = str(value.get("type", "unknown"))
    payload = value.get("payload") if isinstance(value.get("payload"), dict) else value
    sub = payload.get("type")
    if sub and sub != top:
        return f"{top}/{sub}"
    return top


def is_compact_error_value(value: Any) -> bool:
    if isinstance(value, str):
        return is_compact_error_text(value)
    if isinstance(value, list):
        return any(is_compact_error_value(item) for item in value)
    if isinstance(value, dict):
        return any(is_compact_error_value(item) for item in value.values())
    return False


def is_compact_error_text(text: str) -> bool:
    return any(pattern in text for pattern in COMPACT_ERROR_PATTERNS)


def scrub_data_images(value: Any) -> int:
    if isinstance(value, list):
        return sum(scrub_data_images(item) for item in value)
    if isinstance(value, dict):
        total = 0
        for key, item in list(value.items()):
            if isinstance(item, str) and item.startswith("data:image/"):
                value[key] = IMAGE_REPLACEMENT
                total += 1
            else:
                total += scrub_data_images(item)
        return total
    return 0


def wait_for_stable_file(path: Path, seconds: int) -> None:
    first = file_signature(path)
    time.sleep(seconds)
    second = file_signature(path)
    if first != second:
        raise SystemExit(
            "rollout changed during stability check; wait for the active turn to finish or pass --force"
        )


def file_signature(path: Path) -> tuple[int, int]:
    stat = path.stat()
    return stat.st_size, stat.st_mtime_ns


def create_backup_dir(codex_home: Path, thread_id: str) -> Path:
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    backup_dir = codex_home / BACKUP_DIR / f"{stamp}-{thread_id}"
    backup_dir.mkdir(parents=True, exist_ok=False)
    return backup_dir


def backup_sqlite(source: Path, dest: Path) -> None:
    if not source.exists():
        return
    try:
        with sqlite3.connect(source) as src, sqlite3.connect(dest) as dst:
            src.backup(dst)
    except sqlite3.Error as exc:
        print(
            f"warning: sqlite backup failed for {source}; falling back to file copy: {exc}",
            file=sys.stderr,
        )
        shutil.copy2(source, dest)
        for suffix in ("-wal", "-shm"):
            sidecar = Path(str(source) + suffix)
            if sidecar.exists():
                shutil.copy2(sidecar, Path(str(dest) + suffix))


def write_handoff(
    backup_dir: Path, thread_id: str, rollout_path: Path, repaired_lines: list[str]
) -> Path:
    path = backup_dir / "handoff.md"
    with path.open("w", encoding="utf-8") as fh:
        fh.write("# Compaction Repair Handoff\n\n")
        fh.write(f"- Thread ID: `{thread_id}`\n")
        fh.write(f"- Repaired rollout: `{rollout_path}`\n")
        fh.write(
            "- Use this file as continuity context if the repaired thread still "
            "fails after restarting Codex.\n\n"
        )
        fh.write("## Recent Visible Messages\n")
        for raw in repaired_lines[-40:]:
            try:
                value = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if classify_line(value) != "visible_message":
                continue
            role = visible_role(value)
            text = visible_text(value)
            if not text:
                continue
            fh.write(f"\n### {role}\n\n")
            fh.write(text[:3000] + ("\n" if len(text) <= 3000 else "...\n"))
    return path


def visible_role(value: dict[str, Any]) -> str:
    payload = value.get("payload") if isinstance(value.get("payload"), dict) else value
    if payload.get("role"):
        return str(payload["role"])
    if payload.get("type") == "user_message":
        return "user"
    if payload.get("type") == "agent_message":
        return "assistant"
    return "message"


def visible_text(value: Any) -> str:
    pieces: list[str] = []
    collect_text(value, pieces)
    return "\n".join(piece for piece in pieces if not piece.startswith("data:image/"))


def collect_text(value: Any, pieces: list[str]) -> None:
    if isinstance(value, str):
        if value and len(value) < 20_000:
            pieces.append(value)
    elif isinstance(value, list):
        for item in value:
            collect_text(item, pieces)
    elif isinstance(value, dict):
        for key in ("text", "message", "content"):
            if key in value:
                collect_text(value[key], pieces)


def reset_thread_tokens(path: Path, thread_id: str) -> dict[str, Any]:
    if not path.exists():
        return {"old_tokens_used": None, "rows_updated": 0}
    with sqlite3.connect(path) as con:
        row = con.execute(
            "SELECT tokens_used FROM threads WHERE id = ?", (thread_id,)
        ).fetchone()
        old = row[0] if row else None
        cur = con.execute("UPDATE threads SET tokens_used = 0 WHERE id = ?", (thread_id,))
        con.commit()
        return {"old_tokens_used": old, "rows_updated": cur.rowcount}


def redact_sensitive(text: str) -> str:
    text = re.sub(r"sk-[A-Za-z0-9_-]{16,}", "sk-[redacted]", text)
    text = re.sub(
        r"(?i)(Bearer\s+)[A-Za-z0-9._~+/=-]{16,}", r"\1[redacted]", text
    )
    text = re.sub(
        r"(?i)(api[_-]?key[\"'=:\s]+)[A-Za-z0-9._~+/=-]{16,}",
        r"\1[redacted]",
        text,
    )
    return text


if __name__ == "__main__":
    raise SystemExit(main())
