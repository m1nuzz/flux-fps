<#
.SYNOPSIS
Registers the flux-fps logon tasks (elevated helper + unelevated tray).

.DESCRIPTION
Idempotent: existing tasks with the same names are replaced. The helper task
runs flux-fps.exe --helper with RunLevel Highest; the tray task runs the
resident UI with RunLevel Limited. Both use a group principal so they start
for whichever user logs on, without a per-logon UAC prompt.

Requires elevation (run from an elevated MSI or an elevated wizard).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [string]$TaskName = 'FluxFps Helper',
    [string]$TrayTaskName = 'FluxFps Tray',
    [bool]$RegisterTray = $true,
    [string]$UserId = ''
)

$ErrorActionPreference = 'Stop'

$exe = (Resolve-Path -LiteralPath $ExePath -ErrorAction Stop).Path
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw "flux-fps.exe not found: $ExePath"
}

$trigger = New-ScheduledTaskTrigger -AtLogOn

# Restart the helper after a crash; no execution time limit, one instance.
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero)

function Set-FluxFpsTask {
    param(
        [string]$Name,
        [string]$Arguments,
        [string]$RunLevel
    )

    $action = New-ScheduledTaskAction -Execute $script:exe -Argument $Arguments
    if ([string]::IsNullOrWhiteSpace($script:UserId)) {
        # BUILTIN\Users SID: locale-independent, runs at every interactive
        # logon in the logging-on user's context.
        $principal = New-ScheduledTaskPrincipal -GroupId 'S-1-5-32-545' -RunLevel $RunLevel
    }
    else {
        $principal = New-ScheduledTaskPrincipal -UserId $script:UserId `
            -LogonType Interactive -RunLevel $RunLevel
    }

    $existing = Get-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue
    if ($null -ne $existing) {
        Unregister-ScheduledTask -TaskName $Name -Confirm:$false
    }

    Register-ScheduledTask -TaskName $Name -Action $action -Trigger $script:trigger `
        -Principal $principal -Settings $script:settings `
        -Description 'flux-fps logon task' | Out-Null

    Write-Output "Registered scheduled task '$Name': $($script:exe) $Arguments ($RunLevel)"
}

Set-FluxFpsTask -Name $TaskName -Arguments '--helper' -RunLevel 'Highest'
if ($RegisterTray) {
    Set-FluxFpsTask -Name $TrayTaskName -Arguments '' -RunLevel 'Limited'
}
else {
    Write-Output "Tray auto-start is disabled; '$TrayTaskName' was not registered."
}
