# Examples

## ABAB session manifest

`abab-session.toml` is a ready-to-edit 10-run / 5-pair ABAB session manifest.
It contains **no measurement data**; it is a template, not evidence.

```text
mkdir bench
copy examples\abab-session.toml bench\session.toml
flux-fps-master.exe init-session --dir bench --game "CS2"   # alternative: generated template
```

Then, for each run:

```text
flux-fps-master.exe capture --seconds 90 --out bench/run-01.csv
```

Rules enforced by `analyze`:

- `warmup_done = true` after a real 3-5 minute warmup run.
- Exactly alternating `baseline` / `variant`, at least 10 runs (5 pairs).
- At least 60 seconds per run.
- `resolution`, `preset`, `power_plan`, `gpu_driver`, `scenario` and
  `hags_enabled` fixed for the whole session.
- `dpc_max_us` comes from the DPC/ISR monitor; `0` is accepted but weakens the
  mechanism check.

```text
flux-fps-master.exe analyze --dir bench --profile profile.toml
```

Outputs: `bench/bench-report-<date>-<game>.md`,
`bench/frametimes-raw-<date>-<game>.csv` and the verdict stamped into
`profile.toml`. See [`../docs/VALIDATION.md`](../docs/VALIDATION.md) for the
statistics and verdict rules.
