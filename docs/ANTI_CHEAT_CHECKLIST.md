# Anti-cheat and safety checklist

Use this checklist before every release. It is the operational form of the
hard rules in [`../AGENTS.md`](../AGENTS.md) and the security model in
[`../SECURITY.md`](../SECURITY.md).

## 1. Binary boundary matrix

| Capability | `flux-fps.exe` (resident) | `flux-fps-setup.exe` | `flux-fps-master.exe` |
|---|---|---|---|
| Runs at logon | Yes (two per-user tasks) | No | No |
| Elevated | Only the `--helper` process | Operator-run | Operator-run |
| Kernel driver code | **Forbidden** | **Forbidden** | Guardrails only; no driver bundled, access always refused |
| Game process handles | None except `PROCESS_QUERY_LIMITED_INFORMATION` for enumeration | None | None |
| Game memory read/write | **Forbidden** | None | **Forbidden** |
| DLL injection / hooks | **Forbidden** | None | **Forbidden** |
| Process affinity for the game | **Forbidden** (fights Intel Thread Director) | None | Only the tool's own calibration thread |
| CPU Set isolation | Background processes only, via helper | Registry baseline | Calibration produces recommended ids |
| Standby purge | Command 4 only, via helper | None | One responsiveness probe via helper |
| Frametimes | Own ETW Present consumer | None | Own ETW consumer; PresentMon as explicit fallback |
| Updates | Manual check only | None | None |

## 2. CI import-table audit

`scripts/audit-import-table.ps1` parses the PE headers of the built resident
binary directly (no dumpbin / objdump dependency) and fails on any of:

```text
NtLoadDriver, ZwLoadDriver, NtUnloadDriver, ZwUnloadDriver
CreateServiceA/W, OpenSCManagerA/W, StartServiceA/W
NtWriteVirtualMemory, ZwWriteVirtualMemory
NtReadVirtualMemory, ZwReadVirtualMemory
WriteProcessMemory, ReadProcessMemory, VirtualAllocEx
CreateRemoteThread, CreateRemoteThreadEx, NtCreateThreadEx, ZwCreateThreadEx
SetWindowsHookExA/W
```

It also rejects forbidden strings and any NT-prefixed driver image path
(`\??\...sys`). The bare NT object-manager prefix is not treated as a finding
by itself because it also appears in benign toolchain/runtime data.

The audit runs on every push in `.github/workflows/ci.yml` and again in the
release workflow against the release binary. A failure blocks the release.

## 3. Handle and API review

For each change that touches `crates/engine/`:

- [ ] Every `OpenProcess` uses `PROCESS_QUERY_LIMITED_INFORMATION` or the
      minimum required right, and is closed via RAII.
- [ ] No `ReadProcessMemory` / `WriteProcessMemory` / `VirtualAllocEx` /
      `CreateRemoteThread` / `SetWindowsHookEx` anywhere in the workspace.
- [ ] No `NtLoadDriver`, `CreateService*`, `StartService*` or driver path
      construction outside the portable master tool's guardrails module.
- [ ] Isolation targets **background** processes only; the game process is
      never pinned with `SetProcessAffinityMask`.
- [ ] Purge uses only `MemoryPurgeLowPriorityStandbyList` (command 4);
      commands 3 and 5 are not exposed in default config.
- [ ] Every `unsafe` block has a `// SAFETY:` comment.
- [ ] No feature from the project DON'T list (hard GPU IRQ affinity, USB
      interrupt moderation off, RAM disks for shaders, scanline sync,
      WASAPI exclusive mode).

## 4. Anti-cheat coexistence matrix

`guard::ANTI_CHEAT_SERVICES` covers vgc, vgk, BEService, EasyAntiCheat and
FACEIT. The guard applies to the optional kernel path only; the resident app
needs no special case because it performs no kernel work.

| Scenario | Resident behavior | Kernel path |
|---|---|---|
| vgc / vgk (Vanguard) running | Starts normally, user-mode only | Refused, listed in `guard` report |
| BEService / EasyAntiCheat running | Starts normally, user-mode only | Refused, listed in `guard` report |
| FACEIT running | Starts normally, user-mode only | Refused, listed in `guard` report |
| No anti-cheat service | Starts normally | Still refused: no driver is bundled |
| HVCI enabled | No effect on user-mode engine | Listed as a blocking reason |
| Vulnerable driver blocklist not confirmed | No effect on user-mode engine | Listed as a blocking reason |

### Test procedure on an anti-cheat system

The test is **launch and observe**, never memory reads or injection:

1. Install the MSI on a machine with the anti-cheat service active.
2. Confirm `flux-fps.exe` and the `--helper` process start, and the tray UI is
   responsive.
3. Start a protected title and confirm no warnings, no crash and no ban prompt.
4. Confirm `flux-fps-setup.exe --status` shows both logon tasks.
5. Run `flux-fps-master.exe guard` and record the detected services.
6. Run Game Mode on/off and confirm background CPU Set isolation is applied
   and fully reverted.
7. Uninstall and confirm both tasks and the isolation journal are gone.

Record results in [`VALIDATION.md`](VALIDATION.md) before a stable release.

## 5. Kernel opt-in guardrails

For any future work on the optional kernel path, all of the following are
mandatory. None of them is implemented as a load path in `v0.1.0`.

- [ ] Opt-in by default off, behind a dedicated cargo feature; never in the MSI.
- [ ] Explicit owner consent screen describing AV/EDR, anti-cheat flag and
      bugcheck risks.
- [ ] Preflight: HVCI, Smart App Control, vulnerable driver blocklist.
- [ ] Abort when any anti-cheat service from the matrix above is running.
- [ ] Lifecycle `backup -> load -> apply -> unload -> restore`; unload is
      immediate, the driver is never resident and never starts with the system.
- [ ] Full backup/restore snapshot of every changed setting from the same tool.
- [ ] Only whitelisted operations; no arbitrary MSR/MMIO writes.
- [ ] `flux-fps-master guard` remains the single entry point for the decision.
- [ ] `preflight().allowed` stays `false` until a legitimately signed driver
      exists and the owner explicitly enables the feature.

## 6. Release sign-off

- [ ] CI is green on three consecutive runs with the import-table audit passing.
- [ ] Release assets are exactly `flux-fps.msi`, `flux-fps-setup.zip`,
      `flux-fps-master.zip`.
- [ ] Portable tools are not present inside the MSI.
- [ ] Release notes list what was tested, known limitations and what the owner
      should verify.
- [ ] Unsigned builds are labeled as such with a SHA256 for every asset.
- [ ] No private keys, tokens or signing secrets in the repository.
