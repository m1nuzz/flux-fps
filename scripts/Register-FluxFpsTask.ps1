<#
.SYNOPSIS
Registers the elevated flux-fps helper as an at-logon scheduled task.

.DESCRIPTION
Idempotent: an existing task with the same name is replaced. The task runs
flux-fps.exe --helper with RunLevel Highest, so the resident tray process
stays unelevated and the user is not prompted for UAC at every logon.

Requires elevation (run from an elevated MSI or an elevated wizard).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [string]$TaskName = 'FluxFps Helper',
    [string]$UserId = ''
)

$ErrorActionPreference = 'Stop'

$exe = (Resolve-Path -LiteralPath $ExePath -ErrorAction Stop).Path
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw "flux-fps.exe not found: $ExePath"
}

$action = New-ScheduledTaskAction -Execute $exe -Argument '--helper'
$trigger = New-ScheduledTaskTrigger -AtLogOn

if ([string]::IsNullOrWhiteSpace($UserId)) {
    # A group principal runs at every interactive logon in that user's
    # context. The SID is locale-independent (BUILTIN\Users).
    $principal = New-ScheduledTaskPrincipal -GroupId 'S-1-5-32-545' -RunLevel Highest
}
else {
    $principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Highest
}

# Restart the helper after a crash; no execution time limit, one instance.
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero)

$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($null -ne $existing) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
}

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Principal $principal `
    -Settings $settings `
    -Description 'flux-fps elevated helper (per-user Named Pipe bridge)' | Out-Null

Write-Output "Registered scheduled task '$TaskName': $exe --helper"
