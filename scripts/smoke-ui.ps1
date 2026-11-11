<#
.SYNOPSIS
Launches the resident app and asserts that its main window becomes visible.

.DESCRIPTION
Phase 6 UI smoke gate: the resident tray app must actually open a visible
window on a clean machine, not just keep running in the background. The
app's stdout/stderr are captured and always printed, so CI logs show the
engine diagnostics (ETW degradation, game mode errors, reactor faults)
instead of a bare timeout.

Fails with exit code 1 when the app exits early or never shows a window.
#>
[CmdletBinding()]
param(
    [string]$BinaryPath = 'target/debug/flux-fps.exe',
    [int]$TimeoutSeconds = 60
)

$ErrorActionPreference = 'Stop'

$resolved = (Resolve-Path -LiteralPath $BinaryPath -ErrorAction Stop).Path
$workingDirectory = Split-Path -Parent $resolved
$stdoutLog = Join-Path $env:TEMP 'flux-fps-ui-smoke-out.log'
$stderrLog = Join-Path $env:TEMP 'flux-fps-ui-smoke-err.log'

$process = Start-Process -FilePath $resolved `
    -WorkingDirectory $workingDirectory `
    -RedirectStandardOutput $stdoutLog `
    -RedirectStandardError $stderrLog `
    -PassThru
Write-Output "Started flux-fps.exe (pid $($process.Id))"

$windowHandle = [IntPtr]::Zero
try {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ($process.HasExited) {
            throw "app exited early with exit code $($process.ExitCode)"
        }
        $current = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
        if ($null -ne $current -and $current.MainWindowHandle -ne [IntPtr]::Zero) {
            $windowHandle = $current.MainWindowHandle
            break
        }
        Start-Sleep -Milliseconds 500
    }

    if ($windowHandle -eq [IntPtr]::Zero) {
        throw "no visible main window within $TimeoutSeconds s"
    }

    # The window must stay alive for a few seconds; a mount fault often tears
    # the window down right after activation.
    Start-Sleep -Seconds 5
    $current = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
    if ($null -eq $current -or $current.HasExited) {
        throw 'app exited after the window appeared'
    }
    if ($current.MainWindowHandle -eq [IntPtr]::Zero) {
        throw 'main window disappeared while the process was still running'
    }

    Write-Output "PASS: main window is visible (hwnd $windowHandle)"
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -Confirm:$false -ErrorAction SilentlyContinue
    }
    Write-Output '--- app stdout ---'
    Get-Content -LiteralPath $stdoutLog -ErrorAction SilentlyContinue
    Write-Output '--- app stderr ---'
    Get-Content -LiteralPath $stderrLog -ErrorAction SilentlyContinue
}
