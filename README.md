# flux-fps

A Windows 11 game optimizer written in Rust. It tunes the operating system so
games run in a cleaner environment: background CPU Set isolation, low-priority
standby-list purging, ETW-based frametime / DPC telemetry and adaptive frame
pacing.

**Hard invariant:** the installed resident application is user-mode only. It
never reads game memory, never injects a DLL, never loads a driver and never
pins the game process itself. A CI step audits the built PE import table on
every push.

## Three binaries

| Binary | Distribution | Purpose |
|---|---|---|
| `flux-fps.exe` | `flux-fps.msi` / winget | Installed resident tray app, Core Engine, and elevated helper mode (`--helper`). Unelevated for the UI; elevation comes from a Task Scheduler logon task. |
| `flux-fps-setup.exe` | `flux-fps-setup.zip` | Portable, run-once baseline wizard. Registry-backed tweaks, restore point, logon task registration. |
| `flux-fps-master.exe` | `flux-fps-master.zip` | Portable per-PC calibration, raw frametime capture, ABAB statistics, 24 h soak harness and kernel opt-in guardrails. |

The two portable tools are deliberately shipped as separate zip assets and are
**never** bundled into the main MSI.

## What is implemented

- `common` - shared config, telemetry types, helper IPC protocol, per-PC profile.
- `engine` - CPU Set isolation, memory stats and purge, refresh-rate and pacing,
  ETW Present and DPC/ISR consumers, per-user Named Pipe helper bridge,
  crash-safe isolation journal, idempotent privileged operations.
- `ui` - tray app and settings panel (windows-reactor MVU).
- `setup` - baseline wizard with `--apply`, `--restore` and `--status`.
- `master` - calibration, ABAB validation, reports, soak and kernel guardrails.

## Install

Stable releases contain exactly three assets: `flux-fps.msi`,
`flux-fps-setup.zip` and `flux-fps-master.zip`. Releases are manual; nothing is
installed or updated automatically.

```text
winget install m1nuzz.flux-fps
```

Or install `flux-fps.msi` directly. The MSI registers two per-user logon tasks:

- `FluxFps Helper` - `flux-fps.exe --helper`, `RunLevel Highest`.
- `FluxFps Tray` - `flux-fps.exe`, `RunLevel Limited`.

Uninstall removes both tasks and calls the resident restore mode before files
are deleted, so CPU Set isolation is always undone. See
[`docs/USER_GUIDE.md`](docs/USER_GUIDE.md) for the manual script paths.

To check for a newer release without installing anything:

```text
./scripts/Check-FluxFpsUpdate.ps1
```

## Quick start

```text
# 1. One-time baseline (from an elevated prompt)
flux-fps-setup.exe --apply

# 2. Per-PC calibration (portable, no install)
flux-fps-master.exe calibrate --out profile.toml

# 3. A/B validation of the profile
flux-fps-master.exe init-session --dir bench --game "CS2"
flux-fps-master.exe capture --seconds 90 --out bench/run-01.csv
# ...repeat for all 10 ABAB runs, alternating baseline / variant...
flux-fps-master.exe analyze --dir bench --profile profile.toml
```

`analyze` refuses sessions without a warmup, with fewer than 5 ABAB pairs,
with broken alternation, or with runs shorter than 60 s. It prints
`APPLY`, `KEEP-INSIGNIFICANT` or `REVERT` and writes
`bench-report-<date>-<game>.md` plus a combined raw frametime CSV.

## Documentation

- [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md) - installation, daily use, restore,
  updates, troubleshooting.
- [`docs/VALIDATION.md`](docs/VALIDATION.md) - ABAB protocol, statistics,
  verdict rules, report format and the v0.1.0 regression set.
- [`docs/ANTI_CHEAT_CHECKLIST.md`](docs/ANTI_CHEAT_CHECKLIST.md) - anti-cheat
  and safety matrix.
- [`docs/SIGNING.md`](docs/SIGNING.md) - code-signing plan.
- [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) - v0.1.0 beta candidate
  notes, tested scope and owner verification steps.
- [`examples/`](examples/) - ready-to-edit ABAB session manifest.
- [`SECURITY.md`](SECURITY.md) - security model, kernel opt-in policy,
  vulnerability reporting.
- [`AGENTS.md`](AGENTS.md) - repository rules and invariants.

## Status and limitations

- Code signing is documented but not yet wired into CI; unsigned builds will
  trigger a SmartScreen warning.
- Real A/B runs with Deadlock / CS2 and the 24 h soak test require owner
  hardware. The harness, report format and regression checklist are in place;
  no measured results are claimed here.
- No kernel driver is bundled. `flux-fps-master guard` reports the kernel
  preflight and always refuses kernel access.
- Built-in benchmarks usually show a smaller gain than real matches because
  there is less background interference. HAGS must stay fixed across a session.

## Development

The workspace targets Windows and uses `windows-sys` for audited FFI modules.
CI (GitHub Actions, `windows-latest`) runs format, check, clippy with
`-D warnings`, unit tests, a workspace build, installer-script validation, the
PE import-table audit and a WiX v4 MSI authoring build.

```text
cargo fmt --all
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

License: MIT.
