# Flux FPS Agent Guidelines

## Project scope

Flux FPS is a Windows 11 optimizer for games, written in Rust. The product ships as three binaries:

- `flux-fps.exe` - the installed resident application (tray + MVU UI + Core Engine). User-mode only.
- `flux-fps-setup.exe` - portable one-time baseline wizard. User-mode only (registry-backed tweaks).
- `flux-fps-master.exe` - portable per-PC calibration suite (benchmarks, DPC/ISR profiling, A/B gain verification). This is the ONLY binary allowed to contain a kernel module (BYOVD, opt-in, behind the `kernel-setup` cargo feature).

The Cargo workspace contains five crates: `common`, `engine`, `ui`, `setup`, `master`. Shared types live in `common`; all OS-level optimization logic lives in `engine`.

Design documents (MASTER_PLAN, RESEARCH_FINDINGS, PHASES) live OUTSIDE the repository in the local concept folder; do not copy them into the repo unless the owner explicitly asks.

## Anti-cheat and safety invariants (HARD RULES)

1. The resident `flux-fps.exe` binary must never contain driver-loading code, game memory access, or DLL injection. This is enforced by a CI import-table audit.
2. Never suggest process affinity pinning for the game itself (it fights Intel Thread Director); isolation is CPU Sets + memory priority + EcoQoS applied to BACKGROUND processes only.
3. Do not implement features from the project DON'T list: hard GPU IRQ affinity, USB interrupt moderation off, RAM disks for shaders, scanline sync, WASAPI exclusive mode.
4. The standby purge path uses only `MemoryPurgeLowPriorityStandbyList` (command 4). Full standby purge (command 3) and FlushModifiedList (command 5) must not be exposed in default configuration.
5. Kernel access in `master` is opt-in only, with guardrails: blocklist/HVCI pre-check, active anti-cheat service guard (vgc/BEService/EasyAntiCheat/FACEIT abort), load-apply-unload lifecycle, backup/restore. Never bundle it into the installer.

## Repository language and ownership

All Flux-owned source comments, documentation, release notes, and user-facing application strings must be written in English. Do not add Chinese symbols or Chinese comments to Flux-owned code. Conversation with the project owner may use Russian.

Use concise English commit messages. Do not commit scratch notes, generated planning files, temporary screenshots, or unrelated artifacts. Author commits as `m1nuzz <109736024+m1nuzz@users.noreply.github.com>` with no AI attribution trailers.

## Required working process

Work in a deliberate, manual-first loop. Before making a change, inspect the relevant source, existing tests, and design documents. For a multi-step change, create a concrete plan with investigation, implementation, validation, and delivery phases. Do not guess when a repository file, CI log, or Windows smoke result can answer the question.

Make the smallest complete change that solves the observed problem. Preserve existing architecture and avoid broad rewrites. All Win32 FFI lives in small audited modules with `// SAFETY:` comments on every unsafe block. Background threads must not panic; log and degrade instead.

Never claim a fix is ready until the relevant local validation has passed.

## Agent skills

Vendored third-party skills live in `.agents/skills/` (see `.agents/skills/SOURCES.md` for upstream URLs, versions, and licenses). If a task matches a skill, load it with the `skill` tool before implementing.

| Intent | Skill |
|---|---|
| Write or review Rust in `crates/` | `rust-skills` |
| Write, edit, or debug `scripts/*.ps1` | `powershell-expert` |
| Commit changes | `conventional-commit` |
| Pin GitHub Actions or set up Dependabot | `pin-github-actions` |
| Diagnose CI failures from PR logs | `verify-pr-logs` |

## Dependencies and architecture

Keep dependencies pinned where practical. Prefer small, platform-specific changes over broad rewrites. Keep Windows-specific code behind the `engine` crate's FFI modules. The UI must use the `windows-reactor` MVU approach (see the concept docs for fallback plans); do not introduce WebView, Electron, egui, iced, or Tauri.

Engine-to-UI traffic: `tokio::sync::watch` for telemetry (latest value only), bounded `mpsc` with `try_send` for UI commands. The UI thread never calls blocking Win32 APIs.

## Required local validation

Before committing, run the applicable quality gates. The standard local gate is:

```text
cargo fmt --all
cargo fmt --all -- --check
git diff --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Never ignore a failed check merely because it looks unrelated: inspect the log first, distinguish a real regression from a flaky runner, and rerun only when the failure is demonstrably environmental or transient.

## Releases

Releases are manual, never automatic on push. Each release contains three separate assets: `flux-fps.msi`, `flux-fps-setup.zip`, and `flux-fps-master.zip`. Beta releases must use `prerelease: true` and state what changed, what was tested, known limitations, and what the owner should verify. Stable releases require explicit promotion or user instruction.
