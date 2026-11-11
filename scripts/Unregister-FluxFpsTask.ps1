<#
.SYNOPSIS
Removes the flux-fps logon tasks and restores system state.

.DESCRIPTION
Called by the MSI uninstall custom action and by
`flux-fps-setup.exe --restore`. Both the elevated helper task and the
unelevated tray task are removed. Restore is mandatory: any background CPU
Set isolation applied by the helper is undone before the product is removed.
#>
[CmdletBinding()]
param(
    [string]$TaskName = 'FluxFps Helper',
    [string]$TrayTaskName = 'FluxFps Tray',
    [string]$ExePath = '',
    [switch]$SkipRestore
)

$ErrorActionPreference = 'Stop'

function Remove-FluxFpsTask {
    param([string]$Name)

    $existing = Get-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue
    if ($null -ne $existing) {
        Stop-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $Name -Confirm:$false
        Write-Output "Removed scheduled task '$Name'."
    }
    else {
        Write-Output "Scheduled task '$Name' was not present."
    }
}

Remove-FluxFpsTask -Name $TaskName
Remove-FluxFpsTask -Name $TrayTaskName

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
