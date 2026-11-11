# Security Policy

flux-fps changes OS scheduling and memory behavior, so its security boundary
matters more than its feature list. This document describes what the shipped
binaries can and cannot do, and what the project owner must decide explicitly.

## Supported versions

| Version | Supported |
|---|---|
| `v0.1.0` (current development line) | Yes, security fixes only on `main` |
| Older development builds | No |

Fixes are shipped as a new manually created GitHub Release. There is no
automatic updater and no silent install path.

## Trust boundaries

The product is three separate executables. Only the first is installed.

| Component | Trust | Elevation |
|---|---|---|
| `flux-fps.exe` (tray + engine) | Installed, resident | Runs **unelevated** as the interactive user |
| `flux-fps.exe --helper` | Same binary, separate process | Elevated via Task Scheduler `RunLevel Highest` |
| `flux-fps-setup.exe` | Portable, run once | Operator runs it from an elevated prompt |
| `flux-fps-master.exe` | Portable, explicit commands only | Operator decides; no logon task, no service |

The helper is **not** elevated through an embedded `requireAdministrator`
manifest. That would break the unelevated tray process. Elevation comes only
from the `FluxFps Helper` logon task, which is registered per user with a
`Limited` tray task alongside it.

## Anti-cheat and process-handle invariants

The resident binary is user-mode only. These are hard rules enforced in code
review and by the CI import-table audit (`scripts/audit-import-table.ps1`):

- No `NtLoadDriver` / `ZwLoadDriver`, no service creation or start APIs, no NT
  driver image paths.
- No `ReadProcessMemory` / `NtReadVirtualMemory` / `WriteProcessMemory` /
  `NtWriteVirtualMemory`, no `VirtualAllocEx`, no `CreateRemoteThread` /
  `NtCreateThreadEx`.
- No `SetWindowsHookEx` (no DLL injection).
- The only handle opened to another process is
  `PROCESS_QUERY_LIMITED_INFORMATION` for process enumeration and the CPU Set
  operations on **background** processes. Game processes are never opened for
  memory access and never pinned with process affinity; CPU Set isolation is
  applied to background work only, so it does not fight Intel Thread Director.
- Frametimes come from our own ETW `Microsoft-Windows-DxgKrnl` Present
  consumer. No overlay, no in-game hook, no game process instrumentation.
- The standby purge path uses only `MemoryPurgeLowPriorityStandbyList`
  (command 4). Full standby purge (command 3) and `FlushModifiedList`
  (command 5) are not exposed in the default configuration.

See [`docs/ANTI_CHEAT_CHECKLIST.md`](docs/ANTI_CHEAT_CHECKLIST.md) for the
matrices used when validating a build.

## Helper IPC

The unelevated UI talks to the elevated helper over a Named Pipe:

- Pipe name is derived from the current user SID, so one user cannot reach
  another user's helper.
- The pipe DACL is a protected SDDL granting access only to `SYSTEM`,
  Builtin Administrators and that user SID.
- Messages are line-based, capped at 4096 bytes, and parsed into a small
  fixed grammar (`ping`, `purge`, `isolate`, `release`, `restore`,
  `shutdown`). Error text is sanitized before transmission.
- Every helper verb is idempotent. A crash-safe journal in
  `%APPDATA%/flux-fps/helper-state.txt` records pids the helper isolated, so
  `--restore` and the MSI uninstall path can undo isolation even after the
  helper died.

A machine with an active anti-cheat service (vgc / vgk / BEService /
EasyAntiCheat / FACEIT) needs no special handling from the resident app:
it performs no kernel work and no game-process memory access. The guardrails
for optional kernel work live in the portable master tool.

## Kernel opt-in (portable master tool only)

**No kernel driver is bundled with flux-fps in any release asset.** The
optional BYOVD/KDU path described in the design documents would belong
exclusively to `flux-fps-master.exe`, behind a dedicated cargo feature and an
explicit owner opt-in. It is documented here because the risk is real:

- It weakens the host security posture and can trip AV/EDR heuristics.
- It leaves a system-level anti-cheat flag that a one-shot load does not undo.
- It can cause a bugcheck if an MSR/MMIO operation is wrong.
- A legitimately signed driver needs an EV certificate; there is no free
  "white" path for a new driver.

`flux-fps-master guard` reports HVCI, the Microsoft vulnerable driver
blocklist, active anti-cheat services and the refusal reasons.
`request_kernel_access()` always returns `Err(...)`: the `KernelStage` state
machine documents the required `backup -> load -> apply -> unload -> restore`
lifecycle, but there is no load implementation. Kernel access must remain a
deliberate, documented decision by the machine owner, not a build default.
It must never be added to the MSI or to the resident binary.

## Code signing and supply chain

- Signing via SignPath Foundation is documented in
  [`docs/SIGNING.md`](docs/SIGNING.md). It is **not yet wired into CI** and no
  signed build is published at this time.
- Unsigned MSI/EXE downloads trigger a Windows SmartScreen "unknown publisher"
  warning. Verify the SHA256 of a release asset against the release notes
  before running `More info -> Run`.
- Private keys, tokens and signing secrets must never be committed. CI pins
  third-party actions by commit SHA; Dependabot updates are reviewed.
- Release assets are exactly `flux-fps.msi`, `flux-fps-setup.zip` and
  `flux-fps-master.zip`. The portable tools are never bundled into the MSI.

## Updates

`scripts/Check-FluxFpsUpdate.ps1` queries the public GitHub Releases API and
prints the latest version. It does not download, install or execute anything.
There is no auto-update channel, no background downloader and no telemetry
upload.

## Restoration and uninstall

- `flux-fps.exe --restore` removes every CPU Set isolation recorded in the
  journal.
- `flux-fps-setup.exe --restore` reverts the registry-backed baseline.
- Uninstalling the MSI removes both logon tasks and calls the resident restore
  mode before files are deleted.

## Reporting a vulnerability

Open a GitHub issue with the affected binary, version, Windows build, exact
reproduction steps and impact. Do not include crash dumps or logs containing
personal paths. For a privately coordinated fix, request a private channel in
the issue without posting exploit details.
