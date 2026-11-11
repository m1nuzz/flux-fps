<#
.SYNOPSIS
Checks the latest flux-fps GitHub release without downloading or installing it.

.DESCRIPTION
Updates are never applied silently for kernel-adjacent utilities. This script
only reports the newest tag, its release page and the attached assets; the
user installs a new MSI manually.

Exit codes: 0 = check completed, 2 = network/API failure.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$CurrentVersion,
    [string]$Repository = 'm1nuzz/flux-fps'
)

$ErrorActionPreference = 'Stop'

$uri = "https://api.github.com/repos/$Repository/releases/latest"
try {
    $release = Invoke-RestMethod -Uri $uri `
        -Headers @{ 'User-Agent' = 'flux-fps-update-check' } `
        -TimeoutSec 15
}
catch {
    Write-Output "UPDATE_CHECK_FAILED: $($_.Exception.Message)"
    exit 2
}

$latestTag = [string]$release.tag_name
$normalized = $latestTag -replace '^v', ''
$current = $CurrentVersion -replace '^v', ''

Write-Output "Current: $current"
Write-Output "Latest:  $normalized"
Write-Output "Page:    $($release.html_url)"
foreach ($asset in $release.assets) {
    Write-Output "Asset:   $($asset.name)"
}

try {
    $currentVersion = [version]$current
    $latestVersion = [version]$normalized
    if ($latestVersion -gt $currentVersion) {
        Write-Output 'STATUS: update-available (download manually; no automatic install)'
    }
    elseif ($latestVersion -lt $currentVersion) {
        Write-Output 'STATUS: local-build-is-newer'
    }
    else {
        Write-Output 'STATUS: up-to-date'
    }
}
catch {
    Write-Output 'STATUS: version-compare-unavailable (pre-release tag)'
}
