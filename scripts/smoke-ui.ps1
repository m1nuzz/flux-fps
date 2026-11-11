<#
.SYNOPSIS
Launches a windows-reactor app and asserts that its main window becomes
visible.

.DESCRIPTION
Phase 6 UI smoke gate: the app must actually open a visible window, not just
keep running in the background. The app's stdout/stderr are captured and
always printed, so CI logs show engine diagnostics instead of a bare timeout.

On failure the script enumerates every top-level window owned by the process
(handle, class, title, visibility, rect) - this distinguishes "window created
but hidden" from "window never created".

Fails with exit code 1 when the app exits early or never shows a window.
#>
[CmdletBinding()]
param(
    [string]$BinaryPath = 'target/debug/flux-fps.exe',
    [int]$TimeoutSeconds = 60
)

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class WindowFinder
{
    public struct Info
    {
        public IntPtr Handle;
        public string Class;
        public string Title;
        public bool Visible;
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    private delegate bool EnumProc(IntPtr handle, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumProc callback, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr handle, out uint processId);

    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr handle);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetClassName(IntPtr handle, StringBuilder builder, int max);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr handle, StringBuilder builder, int max);

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    private static extern bool GetWindowRect(IntPtr handle, out RECT rect);

    public static List<Info> For(uint processId)
    {
        var found = new List<Info>();
        EnumWindows(
            (handle, lParam) =>
            {
                uint owner = 0;
                GetWindowThreadProcessId(handle, out owner);
                if (owner == processId)
                {
                    var classBuilder = new StringBuilder(256);
                    GetClassName(handle, classBuilder, 256);
                    var titleBuilder = new StringBuilder(256);
                    GetWindowText(handle, titleBuilder, 256);
                    RECT rect;
                    GetWindowRect(handle, out rect);
                    found.Add(new Info
                    {
                        Handle = handle,
                        Class = classBuilder.ToString(),
                        Title = titleBuilder.ToString(),
                        Visible = IsWindowVisible(handle),
                        Left = rect.Left,
                        Top = rect.Top,
                        Right = rect.Right,
                        Bottom = rect.Bottom,
                    });
                }
                return true;
            },
            IntPtr.Zero);
        return found;
    }
}
'@

function Show-ProcessWindows {
    param([uint32]$ProcessId, [string]$Label)
    $windows = [WindowFinder]::For($ProcessId)
    Write-Output "$Label top-level windows owned by pid $ProcessId : $($windows.Count)"
    foreach ($window in $windows) {
        Write-Output (
            "  hwnd={0} class='{1}' title='{2}' visible={3} rect=({4},{5})-({6},{7})" -f `
                $window.Handle, $window.Class, $window.Title, $window.Visible, `
                $window.Left, $window.Top, $window.Right, $window.Bottom
        )
    }
}

$resolved = (Resolve-Path -LiteralPath $BinaryPath -ErrorAction Stop).Path
$workingDirectory = Split-Path -Parent $resolved
$stdoutLog = Join-Path $env:TEMP 'flux-fps-ui-smoke-out.log'
$stderrLog = Join-Path $env:TEMP 'flux-fps-ui-smoke-err.log'

$process = Start-Process -FilePath $resolved `
    -WorkingDirectory $workingDirectory `
    -RedirectStandardOutput $stdoutLog `
    -RedirectStandardError $stderrLog `
    -PassThru
Write-Output "Started $resolved (pid $($process.Id))"

$windowHandle = [IntPtr]::Zero
try {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ($process.HasExited) {
            Show-ProcessWindows -ProcessId $process.Id -Label 'After exit'
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
        Show-ProcessWindows -ProcessId $process.Id -Label 'Timeout:'
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
        Show-ProcessWindows -ProcessId $process.Id -Label 'Vanished:'
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
