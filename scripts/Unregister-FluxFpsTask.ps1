<#
.SYNOPSIS
Removes the elevated flux-fps helper task and restores system state.

.DESCRIPTION
Called by the MSI uninstall custom action and by
`flux-fps-setup.exe --restore`. Restore is mandatory: any background CPU Set
isolation applied by the helper is undone before the product is removed.
#>
[CmdletBinding()]
param(
    [string]$TaskName = 'FluxFps Helper',
    [string]$ExePath = '',
    [switch]$SkipRestore
)

$ErrorActionPreference = 'Stop'

$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($null -ne $existing) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Output "Removed scheduled task '$TaskName'."
}
else {
    Write-Output "Scheduled task '$TaskName' was not present."
}

if ($SkipRestore) {
    Write-Output 'Restore skipped by request.'
    return
}

if ([string]::IsNullOrWhiteSpace($ExePath) -or -not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    Write-Warning 'flux-fps.exe not supplied; isolation journal was not restored.'
    return
}

& $ExePath --restore
if ($LASTEXITCODE -ne 0) {
    Write-Warning "flux-fps --restore exited with code $LASTEXITCODE."
}
else {
    Write-Output 'Isolation state restored.'
}
