<#
.SYNOPSIS
Refreshes the winget manifests for a released MSI.

.DESCRIPTION
winget requires a real SHA256 for every hosted installer, which cannot be
known before the MSI is built. This script is run by the release workflow (or
by the owner before submitting to winget-pkgs): it computes the hash of the
built MSI, rewrites InstallerUrl / InstallerSha256 and stamps PackageVersion
in all three manifests. It never downloads or installs anything.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$MsiPath,
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Repository = 'm1nuzz/flux-fps',
    [string]$WingetDir = ''
)

$ErrorActionPreference = 'Stop'

$resolved = (Resolve-Path -LiteralPath $MsiPath -ErrorAction Stop).Path
$version = $Version.TrimStart('v')
if ($version -notmatch '^\d+\.\d+\.\d+') {
    throw "Version must be a numeric tag such as 0.1.0, got: $Version"
}

if ([string]::IsNullOrWhiteSpace($WingetDir)) {
    $root = Split-Path -Parent $PSScriptRoot
    $WingetDir = Join-Path $root 'packaging/winget'
}
if (-not (Test-Path -LiteralPath $WingetDir -PathType Container)) {
    throw "winget manifest directory not found: $WingetDir"
}

$hash = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
$url = "https://github.com/$Repository/releases/download/v$version/flux-fps.msi"

$manifests = @(
    'm1nuzz.flux-fps.yaml',
    'm1nuzz.flux-fps.installer.yaml',
    'm1nuzz.flux-fps.locale.en-US.yaml'
)
foreach ($name in $manifests) {
    $path = Join-Path $WingetDir $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "missing winget manifest: $path"
    }
    $text = Get-Content -LiteralPath $path -Raw
    $text = $text -replace '(?m)^PackageVersion:.*$', "PackageVersion: $version"
    if ($name -eq 'm1nuzz.flux-fps.installer.yaml') {
        $text = $text -replace '(?m)^\s*InstallerUrl:.*$', "    InstallerUrl: $url"
        $text = $text -replace '(?m)^\s*InstallerSha256:.*$', "    InstallerSha256: $hash"
    }
    [System.IO.File]::WriteAllText($path, $text)
    Write-Output "Updated $name"
}

Write-Output "MSI:      $resolved"
Write-Output "URL:      $url"
Write-Output "SHA256:   $hash"
