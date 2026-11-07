# Exercise the same action queue as the tray menu against a taskbar-minimized window.
param([Parameter(Mandatory)][Diagnostics.Process]$App)
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
        if ($App.HasExited) { throw "Moosik exited while waiting for $Description" }
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 75
    } while ($timer.Elapsed.TotalSeconds -lt $Seconds)
    throw "Timed out waiting for $Description"
}

# Use the player window the installer smoke test has already verified on the taskbar.
$window = $App.MainWindowHandle
try {
    [void][TrayWindow]::ShowWindowAsync($window, 6) # SW_MINIMIZE
    Wait-For { [TrayWindow]::IsIconic($window) } 'Windows to minimize the player' 3
    $App.StandardInput.WriteLine('toggle')
    Wait-For {
        [TrayWindow]::IsWindowVisible($window) -and ![TrayWindow]::IsIconic($window)
    } 'the first tray toggle to restore the player' 15
    $App.StandardInput.WriteLine('toggle')
    Wait-For { ![TrayWindow]::IsWindowVisible($window) } 'the second toggle to hide the player' 6
    $App.StandardInput.WriteLine('quit')
    if (!$App.WaitForExit(10000) -or $App.ExitCode -ne 0) { throw 'Tray Quit failed' }
    Write-Output 'Verified tray restore, hide and quit on Windows.'
} catch {
    Write-Output "Player HWND $window visible=$([TrayWindow]::IsWindowVisible($window)) minimized=$([TrayWindow]::IsIconic($window))"
    Get-Content (Join-Path $env:APPDATA 'moosik/config/moosik.log') -Tail 25 -ErrorAction SilentlyContinue
    throw
} finally {
    if (!$App.HasExited) { Stop-Process -Id $App.Id -Force }
}
