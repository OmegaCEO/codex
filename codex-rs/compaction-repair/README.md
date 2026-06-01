# codex-compaction-repair

Manual recovery tool for local Codex sessions that are wedged by repeated context compaction failures.

The tool is intentionally conservative:

- `inspect` is read-only.
- `repair` is a dry run unless `--apply` is passed.
- `repair --apply` waits for the rollout file to stop changing unless `--force` is used.
- `repair --apply` writes a timestamped backup before modifying anything.
- The rollout repair keeps visible user/assistant context and removes backend-only fat such as tool records, reasoning, compact error tails, token telemetry, and embedded image payloads.

Example:

```powershell
cargo run -p codex-compaction-repair -- inspect --thread-id 019e02bd-5e6e-7df2-b4fd-3ea204753c8f
cargo run -p codex-compaction-repair -- repair --thread-id 019e02bd-5e6e-7df2-b4fd-3ea204753c8f
cargo run -p codex-compaction-repair -- repair --thread-id 019e02bd-5e6e-7df2-b4fd-3ea204753c8f --apply
```

Profiles:

- `gentle`: keep all visible user/assistant messages; remove backend/tool fat. This is the default.
- `standard`: keep 500 visible messages.
- `aggressive`: keep 180 visible messages.
- `emergency`: keep 60 visible messages.

Example:

```powershell
cargo run -p codex-compaction-repair -- repair --thread-id 019e02bd-5e6e-7df2-b4fd-3ea204753c8f --strategy auto
cargo run -p codex-compaction-repair -- repair --thread-id 019e02bd-5e6e-7df2-b4fd-3ea204753c8f --profile aggressive
```

After a successful applied repair, fully quit and reopen Codex Desktop before resuming the thread.
