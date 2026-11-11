# Release notes

## v0.1.0-beta.1 (candidate, not yet promoted)

Status: **candidate**. Tagging a beta tag is a manual action by the repository
owner. A stable `v0.1.0` requires the manual checklist in
[`VALIDATION.md`](VALIDATION.md) to be completed.

### What changed

- Three-binary architecture: installed resident tray/Core Engine, portable
  baseline wizard, portable per-PC calibration and A/B validation suite.
- Elevated helper in the same resident binary (`--helper`) started by a
  Task Scheduler logon task, with a per-user Named Pipe ACL, idempotent verbs
  and a crash-safe isolation journal.
- ETW `Microsoft-Windows-DxgKrnl` Present frametime monitor with bounded raw
  recording for full A/B runs; PresentMon as an explicit fallback.
- ABAB harness: Welch's t-test, Mann-Whitney U, 10k-resample bootstrap 95% CI,
  `APPLY` / `KEEP-INSIGNIFICANT` / `REVERT` verdict, Markdown report and
  combined raw frametime CSV.
- Per-PC profile TOML, calibration sweep, profile comparison, 24 h soak
  harness, helper kill/restore regression test and kernel opt-in guardrails.
- Resident settings panel shows the last calibration verdict.
- WiX v4 MSI, winget manifests, update-check script, SignPath documentation,
  release workflow publishing exactly three assets.
- CI gates: format, check, clippy `-D warnings`, unit tests, locked build,
  script/XML/manifest validation, PE import-table audit, MSI authoring build.

### What was tested

Only the automated regression set on GitHub Actions (`windows-latest`):
format, compile, clippy, unit tests, locked workspace build, installer script
and XML parsing, winget manifest fields, resident import-table audit and a
WiX v4 MSI authoring build. See the CI run log in `VALIDATION.md`.

The release pipeline itself was validated as a **build-only dry run**
(`workflow_dispatch` with `publish = false`, run 43076877159): release build
with `--locked`, infrastructure validation, resident PE import-table audit,
WiX v4 CLI install, MSI production, both portable zips, the winget-free
prerelease path and release-notes generation. No public release was created;
publishing requires an explicit tag push by the repository owner.

### Known limitations

- Builds are unsigned until SignPath is configured. SmartScreen will warn;
  verify the MSI SHA256 from the release notes before running.
- No real A/B runs with Deadlock / CS2, no 24 h soak, no anti-cheat-system
  launch test and no MSI install/uninstall smoke test have been performed by
  the maintainers yet. These require owner hardware.
- No kernel driver is bundled and kernel access is always refused.
- The winget manifest in the repository contains a placeholder
  `InstallerSha256`; the release workflow refreshes the runner copy from the
  built MSI.
- Built-in benchmarks show smaller gains than real matches; HAGS must stay
  fixed across a session.

### What the owner should verify

1. Install the MSI, confirm both logon tasks and the unelevated tray.
2. Run `flux-fps-setup.exe --apply`, `--status` and `--restore`.
3. Run `flux-fps-master.exe calibrate` and copy the profile to
   `%APPDATA%\flux-fps\profile.toml`; confirm the settings panel shows it.
4. Run the ABAB workflow on Deadlock and CS2 (5 pairs each, >= 60 s per run).
5. Run the 24 h soak and `restore-test`; confirm no ETW session leak.
6. Launch on a system with EAC / Vanguard active and record the result in the
   anti-cheat checklist.
7. Uninstall the MSI and confirm CPU Set isolation is fully reverted.
