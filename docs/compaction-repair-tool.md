# Codex Compaction Repair Tool

This fork contains a standalone manual repair tool for local Codex sessions that
are stuck on repeated context compaction failures.

Use it when a session repeatedly reports errors such as:

- `Context automatically compacted`
- `Error running remote compact task`
- `Failed to run pre-sampling compact`
- `/responses/compact`

## Immediate Tool

The immediately runnable tool is:

```powershell
python scripts/codex_compaction_repair.py inspect --thread-id <THREAD_ID>
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID>
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --apply
```

`repair` is a dry run unless `--apply` is provided.

The default repair profile is `gentle`, which keeps all visible user/assistant
messages and removes backend/tool fat first. `standard` keeps the latest 500
visible messages. Profiles run from least destructive to most destructive:

| Profile | Visible messages | Turn context | Event markers | Use when |
| --- | ---: | ---: | ---: | --- |
| `gentle` | all | 40 | 160 | First attempt when the rollout mostly needs backend/tool fat removed. |
| `standard` | 500 | 20 | 80 | Default for active coding sessions. |
| `aggressive` | 180 | 10 | 40 | Standard still leaves the session too large. |
| `emergency` | 60 | 4 | 12 | Last resort before starting a fresh chat from handoff. |

The default strategy is `manual`, which applies one selected profile. The
`auto` strategy starts at the selected profile, defaults to `gentle`, and steps
down toward `emergency` until the repaired rollout is under the target byte
budget.

```powershell
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --strategy auto
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --strategy auto --target-bytes 350000
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --strategy auto --profile standard
```

With `--apply`, the tool:

1. Locates the target rollout JSONL from `state_5.sqlite` or `~/.codex/sessions`.
2. Waits for the rollout file to remain unchanged for 10 seconds.
3. Creates a timestamped backup under `~/.codex/session-slim-backups`.
4. Backs up the rollout JSONL and SQLite state.
5. Removes backend-only rollout fat:
   - compacted records
   - compact error tails
   - tool/function/shell/web records
   - reasoning records
   - older visible messages beyond the configured retention window
   - embedded `data:image` payloads
6. Preserves recent visible user/assistant messages, recent turn context, event
   markers, and first/last `session_meta`.
7. Writes a local `handoff.md` in the backup folder.
8. Resets `threads.tokens_used` to `0` unless `--skip-token-reset` is passed.

After an applied repair, fully quit and reopen Codex Desktop before resuming the
session. Stale in-memory context can continue failing even after the local files
are repaired.

## Useful Options

```powershell
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --keep-all-visible-messages
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --profile gentle
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --profile standard
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --profile aggressive
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --profile emergency
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --keep-visible-messages 400
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --keep-turn-context 30 --keep-event-markers 120
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --apply --force
python scripts/codex_compaction_repair.py repair --thread-id <THREAD_ID> --apply --skip-token-reset
```

Use `--force` only after confirming the stuck session is no longer writing to
the rollout file.

## Native Rust Direction

A native Rust binary crate has been started at:

```text
codex-rs/compaction-repair
```

The intended native command is:

```powershell
cargo run -p codex-compaction-repair -- inspect --thread-id <THREAD_ID>
cargo run -p codex-compaction-repair -- repair --thread-id <THREAD_ID> --apply
```

That crate mirrors the Python tool behavior and is the path to later CLI/Windows
integration.

## Local Rust Build Requirements

On Windows, Rust is not enough by itself for the default MSVC target. Local
builds also need Microsoft C++ Build Tools so `link.exe` is available.

Required pieces:

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
winget install Microsoft.VisualStudio.2022.BuildTools
```

For the Visual Studio installer, include the C++ build tools workload:

```text
Microsoft.VisualStudio.Workload.VCTools
```

After installation, open a new shell or use a Developer PowerShell, then run:

```powershell
cd codex-rs
cargo fmt -p codex-compaction-repair
cargo test -p codex-compaction-repair
cargo run -p codex-compaction-repair -- repair --thread-id <THREAD_ID>
```

Local verification status for this branch:

- Rust installed with `rustup`.
- Visual Studio Build Tools 2022 installed with the C++ tools workload.
- `cargo fmt -p codex-compaction-repair` runs successfully.
- `cargo test -p codex-compaction-repair` passes.
- `cargo build -p codex-compaction-repair` succeeds.

## CLI Integration Plan

The first native CLI integration should expose the same engine under `codex
debug`:

```powershell
codex debug compaction-repair inspect --thread-id <THREAD_ID>
codex debug compaction-repair repair --thread-id <THREAD_ID> --strategy auto --apply
```

Implementation outline:

1. Split `codex-rs/compaction-repair/src/main.rs` into a small binary plus a
   reusable library module.
2. Add a `codex-compaction-repair` workspace dependency to `codex-rs/cli`.
3. Add `DebugSubcommand::CompactionRepair` in `codex-rs/cli/src/main.rs`.
4. Route inspect/repair arguments to the shared repair library.
5. Keep `--apply` explicit and keep dry-run as the default.

## Local Windows Desktop Clone Plan

The installed Windows package has two important executable layers:

- `app/Codex.exe`: Electron/Owl desktop shell.
- `app/resources/codex.exe`: Rust Codex core used by the desktop shell.

Because the public repo does not currently include the Electron app source, the
practical local desktop experiment is:

1. Copy the installed `OpenAI.Codex_...\app` directory into a dev folder.
2. Build the forked Rust `codex.exe`.
3. Replace only the copied dev folder's `resources/codex.exe`.
4. Launch the copied `app/Codex.exe` with an isolated `CODEX_HOME` for testing.
5. Keep the Microsoft Store/OpenAI-installed app untouched.

This avoids modifying the installed Codex instance while still testing the
desktop wrapper against a patched Rust core.

For OpenAI updates over time, treat the copied desktop runtime as disposable:

1. Pull/rebase the fork against `openai/codex`.
2. Rebuild the forked Rust core.
3. Refresh the copied desktop runtime from the latest installed OpenAI package.
4. Swap in the rebuilt `resources/codex.exe`.
5. Run compaction regression/recovery tests before using it for real work.
