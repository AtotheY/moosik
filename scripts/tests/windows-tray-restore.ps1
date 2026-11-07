# Exercise the same action queue as the tray menu against a taskbar-minimized window.
param([Parameter(Mandatory)][string]$Exe)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class TrayWindow {
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
}
'@

function Wait-For([scriptblock]$Condition, [string]$Description, [int]$Seconds) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ($app.HasExited) { throw "Moosik exited while waiting for $Description" }
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 75
    } while ($timer.Elapsed.TotalSeconds -lt $Seconds)
    throw "Timed out waiting for $Description"
}

$start = [Diagnostics.ProcessStartInfo]::new($Exe, '--tray-test stdin')
$start.UseShellExecute = $false
$start.RedirectStandardInput = $true
$app = [Diagnostics.Process]::Start($start)
try {
    Wait-For { $app.Refresh(); $app.MainWindowHandle -ne 0 } 'the player window' 30
    $window = $app.MainWindowHandle
    [void][TrayWindow]::ShowWindowAsync($window, 6) # SW_MINIMIZE
    Wait-For { [TrayWindow]::IsIconic($window) } 'Windows to minimize the player' 3
    $app.StandardInput.WriteLine('toggle')
    Wait-For {
        [TrayWindow]::IsWindowVisible($window) -and ![TrayWindow]::IsIconic($window)
    } 'the first tray toggle to restore the player' 15
    $app.StandardInput.WriteLine('toggle')
    Wait-For { ![TrayWindow]::IsWindowVisible($window) } 'the second toggle to hide the player' 6
    $app.StandardInput.WriteLine('quit')
    if (!$app.WaitForExit(10000) -or $app.ExitCode -ne 0) { throw 'Tray Quit failed' }
    Write-Output 'Verified tray restore, hide and quit on Windows.'
} finally {
    if (!$app.HasExited) { Stop-Process -Id $app.Id -Force }
}
