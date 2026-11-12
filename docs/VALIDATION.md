# Validation and A/B protocol

This document defines how a profile is proven (or disproven) on a specific PC,
what `flux-fps-master` computes, and the regression set that must be green
before tagging `v0.1.0`.

## 1. What is being measured

The project targets stutter, not average FPS. Metrics, in priority order:

| Metric | Why |
|---|---|
| 1% low / 0.1% low FPS | Main target: the worst frames, derived from P99 / P99.9 frametime |
| Frametime variance (sigma), P95 / P99 frametime | Direct measure of frame-time smoothness |
| Average FPS | Secondary; rarely moves because we remove interference rather than overclock |
| Max DPC microseconds per core | Mechanism check: did the profile actually reduce interference? |

A report without a 1% low gain above the threshold is **not** a win, even if
average FPS rose.

## 2. ABAB protocol

1. **Warmup run** of 3-5 minutes, not counted: thermal soak and shader cache.
   The first run is always slow.
2. **Fix the variables:** resolution, graphics preset, power plan, GPU driver
   version, background conditions and HAGS. The single differing variable is
   Game Mode OFF (baseline) vs Game Mode ON with the calibrated profile
   (variant). HAGS distorts GPU-side PresentMon fields, so its state is fixed
   and logged, and GPU-side fields are not compared.
3. **Alternate ABAB**, never AA+BB. At least **5 pairs / 10 measured runs**,
   which neutralizes thermal and background drift over time.
4. **Scene:** built-in benchmark > replay/demo > fixed route. Each run is
   at least **60 s**. For games without a benchmark, use a replay or training
   scenario of fixed length.
5. **Capture:** our own ETW `Microsoft-Windows-DxgKrnl` Present consumer
   (zero-touch, no overlay, no game process handles). PresentMon CLI is an
   explicit fallback and must be supplied by the operator.

`session::validate()` enforces the schema version, warmup flag, pair count,
exact alternation and minimum run duration before any statistic is computed.
`session::write_template()` generates the 10-run skeleton.

## 3. Statistics

All of it is pure Rust in `crates/master/src/stats.rs`, with no external
statistics crate, so CI can unit-test it deterministically.

- **Welch's t-test** on the frametime distributions (unequal variances),
  p-value from the regularized incomplete beta function.
- **Mann-Whitney U** on the per-run 1% lows, with average ranks for ties and a
  tie-corrected normal approximation. The 1% low distribution is heavy-tailed,
  so a nonparametric test is the decision gate.
- **Bootstrap 95% CI** of the 1% low gain, 10,000 resamples, seeded with a
  deterministic xorshift64* so results are reproducible run to run.
- Frametimes are sanitized (finite, > 0, < 1000 ms); longer deltas are
  treated as stream discontinuities such as alt-tab or loading, not frames.

## 4. Verdict rules

Defaults in `DecisionConfig`: `min_gain_pct = 5.0`, `alpha = 0.05`,
`resamples = 10_000`.

| Verdict | Condition |
|---|---|
| `APPLY` | 1% low gain >= +5%, Mann-Whitney p < 0.05, bootstrap CI lower bound > 0 |
| `KEEP-INSIGNIFICANT` | Gain in the gray zone (0-5%) or not statistically verified; keep the profile but do not claim a win |
| `REVERT` | 1% low regressed (gain < 0), or max DPC increased by more than 10% (mechanism check failed) |

`analyze --dir <dir> --profile <file>` stamps the verdict into
`profile.toml` as `last_verdict`. The UI can then show the last verified gain.

## 5. Artifacts

For a session in `bench/`, `analyze` writes:

| File | Contents |
|---|---|
| `bench/bench-report-<date>-<game>.md` | Fixed variables, per-run metrics, statistics table, verdict and honest limitations |
| `bench/frametimes-raw-<date>-<game>.csv` | Combined raw frametimes: `run_id,variant,frametime_ms` |
| `profile.toml` | Schema-versioned profile with `last_verdict` (only when `--profile` is passed) |
| `bench/session.toml` | Operator-maintained manifest; history is kept locally |

The CSV reader accepts our `frametime_ms` column, PresentMon's
`MsBetweenPresents`, or a headerless single numeric column.

### Report layout (illustrative; no measured values)

```text
# A/B validation report: <game> (YYYY-MM-DD)

## Fixed variables
| Variable | Value |
|---|---|
| Scenario | built-in benchmark |
| Resolution | 1920x1080 |
| Graphics preset | high |
| Power plan | balanced |
| GPU driver | <version> |
| HAGS | on |
| Warmup | done |
| Runs | 10 (ABAB, 5 pairs) |

## Per-run metrics
| Run | Variant | Seconds | Frames | Avg FPS | 1% low | 0.1% low | P99 ms | sigma ms | DPC max us |

## Statistics
| 1% low gain | Bootstrap 95% CI | Welch p | Mann-Whitney U p | Max DPC reduction |

## Verdict: APPLY | KEEP-INSIGNIFICANT | REVERT
```

## 6. Honest limitations

- Real games are non-deterministic; ABAB alternation and a nonparametric test
  reduce but do not eliminate drift.
- Built-in benchmarks usually show a smaller gain than real matches because
  there is less background noise. Stutter targets remain visible in P99.
- HAGS distorts some GPU-side ETW fields, so the primary metric is frametime,
  not GPU timings.
- `Avg FPS` is reported but never used as the decision gate.

## 7. Regression set for v0.1.0

Exit criteria: the automated regression set must be green on **at least three
CI runs**, and the owner-side manual checklist must be completed before a
stable release is promoted. A ready-to-edit session manifest lives in
[`../examples/abab-session.toml`](../examples/abab-session.toml); CI parses it
and asserts that it satisfies the ABAB rules.

### Automated (GitHub Actions, every push)

| Check | Command / step |
|---|---|
| Formatting | `cargo fmt --all -- --check` |
| Compile | `cargo check --workspace` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Unit tests | `cargo test --workspace` (stats, session, CSV, guard state machine, soak summary, IPC, journal, pacing, etc.) |
| Locked build | `cargo build --workspace --locked` |
| Infrastructure | script parsing, XML parsing, winget manifests |
| Import-table audit | no driver-loading / injection imports in `flux-fps.exe` |
| WiX v4 MSI | authoring validation build produces an MSI |
| Minimal reactor window smoke | `scripts/smoke-ui.ps1` on the `smoke_window` example: visible window handle |
| Tray UI window smoke | `scripts/smoke-ui.ps1` on `flux-fps.exe`: window visible and alive after 5 s |

### Owner-side manual checklist (requires real hardware)

| # | Test | Expected result | Status |
|---|---|---|---|
| 1 | ABAB, Deadlock, replay/fixed scenario, 5 pairs | Report produced with a verdict | Not run |
| 2 | ABAB, CS2 built-in benchmark, 5 pairs | Report produced with a verdict | Not run |
| 3 | Synthetic load without AVX throttling | No false-positive gain from clock drift | Not run |
| 4 | Profile comparison after a GPU driver update | `compare` shows expected deltas | Not run |
| 5 | 24 h soak | `helper_failures = 0`, `etw_sessions` stable, `restore_ok = true` | Not run |
| 6 | Kill the elevated helper, then `restore-test` | Isolation journal empty after restore | Not run |
| 7 | Launch on a system with EAC / Vanguard active | Resident starts, no game memory access, no kernel load | Not run |
| 8 | MSI install -> use -> uninstall | Both tasks removed, isolation restored | Not run |

**No measured benchmark results are claimed in this repository yet.** The
harness, report format and checklist are implemented; the runs above must be
executed by the machine owner and attached to the release notes.

The built-in ETW capture path itself was verified locally on the owner's
Windows 11 machine with HAGS enabled: `flux-fps-master capture --seconds 6`
recorded 1915 `Present_Info` deltas with no PID filter, and a filtered
`--pid` run recorded 540 deltas in 5 s from one presenting process. These are
capture-mechanism checks, **not** A/B benchmark results. Without `--pid`, all
presenting processes on the desktop contribute to the frametime stream, so
A/B runs must always pass the game's pid.

### CI run log

Verified green runs of the full pipeline (including the import-table audit
and the WiX v4 MSI authoring build where present):

| # | Run ID | Commit | Result |
|---|---|---|---|
| 1 | 42927739115 | a26c82d | success |
| 2 | 42931467526 | 004e4e9 | success |
| 3 | 43073774823 | 411e7be | success (full pipeline including MSI authoring) |
| 4 | 43074436389 | 3caaec2 | success |
| 5 | 43076652323 | b88ed16 | success |
| 6 | 43083583293 | 32ad247 | success (UI window deadlock fix, both smoke gates green) |
| 7 | 43123469527 | 3ca04d2 | success (two-column UI redesign, both smoke gates green) |

The first two predate the MSI authoring gate; the third is the first run of
the complete Phase 5/6 pipeline. In addition, the release pipeline was
validated end to end as a build-only dry run (`workflow_dispatch`,
`publish = false`, run 43076877159) without creating a public release.

Add further green run IDs here as they land.

Do not tag `v0.1.0` as stable until the manual checklist above is complete.
Beta releases may be tagged as prerelease with the untested items explicitly
listed in [`RELEASE_NOTES.md`](RELEASE_NOTES.md).
