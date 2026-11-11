# Code signing

Release binaries are currently **unsigned**. Windows SmartScreen will warn on
download until signing is configured. This document describes the intended
setup; no certificate, private key or API token is stored in this repository.

## Why signing matters here

flux-fps is a kernel-adjacent utility: it starts an elevated helper at logon
and applies system-wide scheduling changes. An unsigned binary is both a
usability problem (SmartScreen) and a trust problem (users cannot verify that
the helper was built from this repository). Signing is therefore a release
requirement, not a cosmetic step.

## Provider

[SignPath Foundation](https://signpath.org/) offers free code signing for
open-source projects. SignPath holds the certificate and performs signing in
its own infrastructure, so no private key ever reaches GitHub Actions.

Prerequisites:

1. A public repository with an open-source license.
2. A SignPath account, an organization, a project and a signing policy.
3. A CI token stored as a GitHub Actions secret named `SIGNPATH_API_TOKEN`.
   The organization, project and policy slugs are also stored as secrets (or
   as repository variables - they are not credentials).
4. The release workflow must build the artifacts and hand them to SignPath;
   the signed artifacts are what get attached to the GitHub Release.

## What gets signed

| Artifact | Signed |
| --- | --- |
| `flux-fps.exe` (resident, inside `flux-fps.msi`) | yes |
| `flux-fps.msi` (installer package) | yes |
| `flux-fps-setup.exe` (portable wizard) | yes |
| `flux-fps-master.exe` (portable calibrator) | yes |

The installer must be signed **after** the MSI is built, and the executable
inside it must be signed **before** the MSI is built. Building order in the
release workflow is therefore: build binaries -> sign binaries -> build MSI
from signed binaries -> sign MSI -> publish.

## CI rules

- Never commit certificates, `.pfx` files, passwords or API tokens.
- Never skip signing silently: if the signing step is unavailable, the
  release job must fail rather than publish unsigned artifacts under a stable
  tag.
- Use short-lived CI tokens and restrict the SignPath policy to the release
  branch/tag pattern.
- Record the certificate subject and the artifact SHA256 in the release notes
  so users can verify what they downloaded.
- Beta tags keep `prerelease: true` even when signed.

## Manual verification

```powershell
Get-AuthenticodeSignature .\flux-fps.msi | Format-List Status, StatusMessage, SignerCertificate
Get-FileHash .\flux-fps.msi -Algorithm SHA256
```

The SHA256 printed here must match the one in the release notes and in
`packaging/winget/m1nuzz.flux-fps.installer.yaml` (refreshed by
`scripts/Update-WingetManifest.ps1`).

## Updates

The resident application never installs updates on its own.
`scripts/Check-FluxFpsUpdate.ps1` only queries the GitHub Releases API and
prints the latest tag and assets. The user downloads a signed MSI manually.
Portable tools are updated manually as well.
