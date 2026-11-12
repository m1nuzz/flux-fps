# flux-fps user guide

This guide covers installation, daily use, calibration, A/B validation,
uninstallation and recovery. Everything is manual: flux-fps never updates or
installs itself.

## 1. Requirements

- Windows 11, x64.
- A per-user logon (the helper task is registered for the interactive user).
- An elevated prompt is needed **once** for `flux-fps-setup.exe --apply` or for
  the MSI install. The tray app itself runs unelevated.

## 2. Install the resident app

Preferred:

```text
winget install m1nuzz.flux-fps
```

Or run `flux-fps.msi` from a GitHub Release. The installer places
`flux-fps.exe` and the two Task Scheduler scripts under
`Program Files\flux-fps`, then registers:

| Task | Command | Run level |
|---|---|---|
| `FluxFps Helper` | `flux-fps.exe --helper` | `Highest` |
| `FluxFps Tray` | `flux-fps.exe` | `Limited` |

Both use an `-AtLogOn` trigger, `RestartCount 3` with a 1 minute restart
interval, and `MultipleInstances IgnoreNew`. The helper restarts after a crash;
the tray does not need elevation.

Manual registration (equivalent to what the MSI does):

```text
./scripts/Register-FluxFpsTask.ps1 -ExePath "C:\Program Files\flux-fps\flux-fps.exe" -RegisterTray:$true
```

Pass `-RegisterTray:$false` when `auto_start` is disabled in the config: the
elevated helper task is still required, but no tray autostart is created.

Check state:

```text
flux-fps-setup.exe --status
./scripts/Unregister-FluxFpsTask.ps1   # only when removing manually
```

## 3. One-time baseline

From an elevated prompt, run the portable wizard once:

```text
flux-fps-setup.exe --apply
```

It writes the default config and registers both logon tasks.
`flux-fps-setup.exe --status` shows config, auto-start and task state.
`flux-fps-setup.exe --restore` removes both tasks and restores isolation state.

Changes to the `auto_start` toggle in the settings panel are saved to config;
re-run `flux-fps-setup --apply` from an elevated prompt to update the tray
logon task.

## 4. Daily use

The tray app runs the Core Engine. Game Mode applies CPU Set isolation and
memory priority to **background** processes only; it never pins the game and
never opens game memory. Low-priority standby pages are purged through the
elevated helper with command 4 only.

The settings panel shows telemetry, pacer advice, config toggles and whether
the elevated helper pipe is reachable. If the helper is unavailable, privileged
operations degrade to user-mode behavior instead of failing the UI.

## 5. Per-PC calibration

Run `flux-fps-master.exe` from the portable zip, from any directory. No install,
no service.

```text
flux-fps-master.exe --help
flux-fps-master.exe --version
flux-fps-master.exe calibrate --out profile.toml
```

`calibrate` benchmarks each logical processor on the tool's own thread, finds
the safe maximum refresh rate, selects E-core CPU Set ids for background work,
and measures standby-purge responsiveness. It writes a schema-versioned
`profile.toml` with `pacing_safety_factor` and an empty `last_verdict`.

The resident app reads `%APPDATA%\flux-fps\profile.toml`. Save or copy the
calibrated profile there, then use **Refresh calibration status** in the
settings panel; it shows the last A/B verdict. A missing or incompatible
profile is normal and falls back to default config values.

After a GPU driver or application update, compare the old and new profiles:

```text
flux-fps-master.exe calibrate --out profile-new.toml
flux-fps-master.exe compare --old profile.toml --new profile-new.toml
```

## 6. A/B validation workflow

Full protocol and statistics are in [`VALIDATION.md`](VALIDATION.md). The
short version:

```text
flux-fps-master.exe init-session --dir bench --game "CS2"
```

Edit `bench/session.toml`: fix resolution, preset, power plan, GPU driver and
HAGS, set `warmup_done = true` after a 3-5 minute warmup, and keep the 10 ABAB
runs (5 pairs, alternating baseline/variant, at least 60 s each).

Capture each run while the game or its built-in benchmark is running:

```text
flux-fps-master.exe capture --seconds 90 --pid <game pid> --out bench/run-01.csv
```

`--pid <n>` restricts the ETW Present consumer to one process. **Always pass
the game's pid** for A/B runs: without a filter, `Present_Info` events from
every presenting process on the desktop (video players, browsers, the shell)
are merged into one frametime stream and the 1% low becomes meaningless. The
pid comes from the presenting process itself, not from `dwm.exe`. If the ETW
session cannot start, supply PresentMon explicitly:

```text
flux-fps-master.exe capture --seconds 90 --out bench/run-01.csv --presentmon PresentMon.exe --process cs2.exe
```

Then:

```text
flux-fps-master.exe analyze --dir bench --profile profile.toml
```

The tool refuses invalid sessions, prints the statistics, writes
`bench/bench-report-<date>-<game>.md` and
`bench/frametimes-raw-<date>-<game>.csv`, and stamps the verdict into the
profile when `--profile` is given. Verdicts are `APPLY`,
`KEEP-INSIGNIFICANT` and `REVERT`.

## 7. Soak and recovery tests

Both commands are explicit and are intended for the owner's validation run,
not for unattended machines.

```text
flux-fps-master.exe soak --hours 24 --interval 60 --dir soak
flux-fps-master.exe restore-test
```

`soak` writes a CSV of helper reachability, isolation journal size, memory
pressure and our own ETW session count, then restores state and reports
failures. A growing `etw_sessions` value indicates a leaked ETW session.
`restore-test` kills the elevated helper, waits for the scheduled task to
restart it, and verifies that the isolation journal is empty after a full
restore.

To restore manually at any time:

```text
flux-fps.exe --restore
flux-fps-setup.exe --restore
```

## 8. Uninstall

Uninstalling the MSI stops and removes both tasks, invokes the resident
restore mode, and only then deletes files. CPU Set isolation is therefore
undone even if the helper died earlier (the journal is used as a fallback).

## 9. Updates

```text
./scripts/Check-FluxFpsUpdate.ps1
```

This prints the latest published version. It does not download or install
anything. Download release assets manually and verify their SHA256 against the
release notes. Unsigned builds trigger a SmartScreen warning; see
[`SIGNING.md`](SIGNING.md).

## 10. Kernel access

```text
flux-fps-master.exe guard
```

Prints HVCI, vulnerable driver blocklist and anti-cheat service status, and
explains why kernel access is refused. No driver is bundled, and the portable
master tool never loads one. See [`../SECURITY.md`](../SECURITY.md).

## Troubleshooting

| Symptom | What to check |
|---|---|
| Settings panel says helper unavailable | `flux-fps-setup.exe --status`; re-run `--apply` elevated; check the `FluxFps Helper` task in Task Scheduler. |
| Purge reports no gain | Helper elevation is missing, or the system has no priority-0 standby pages to purge. |
| `capture` returns no Present events | Run elevated with a process actively presenting frames. Use `--pid <game pid>` to isolate one game; without it frametimes from every presenting process (desktop, player, browser) are mixed. DWM's pid is not the presenting pid for `Present_Info`. As a last resort use `--presentmon` + `--process`. Keep HAGS fixed. |
| `analyze` rejects a session | Warmup flag, at least 10 runs, exact ABAB alternation, and >= 60 s per run are all mandatory. |
| Tray does not start at logon | `config.auto_start` must be true and `--apply` must be re-run elevated. |
| SmartScreen warning | Build is unsigned; verify SHA256 before proceeding. |
