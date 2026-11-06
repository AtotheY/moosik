# Manual pin on a disposable interactive Windows runner, then upgrade the installed app.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Mouse {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint dx,uint dy,uint data,UIntPtr extra);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls,string title);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[Mouse]::SetProcessDPIAware() | Out-Null
$barHandle=[Mouse]::FindWindow('Shell_TrayWnd',$null)
if (!$barHandle.ToInt64()) { throw 'No interactive Explorer taskbar on this runner' }
$bar=[System.Windows.Automation.AutomationElement]::FromHandle($barHandle)
$all=[System.Windows.Automation.Condition]::TrueCondition
$scope=[System.Windows.Automation.TreeScope]::Descendants
function Click-Element($el,$right) {
  $r=$el.Current.BoundingRectangle
  Write-Host "Click $($el.Current.Name) ($r)"
  if ($r.IsEmpty -or $r.Width -le 0) {throw 'no rectangle'}
  [Mouse]::SetCursorPos([int]($r.Left+$r.Width/2),[int]($r.Top+$r.Height/2)) | Out-Null
  if ($right) { $down=8;$up=16 } else { $down=2;$up=4 }
  [Mouse]::mouse_event($down,0,0,0,[UIntPtr]::Zero)
  Start-Sleep -Milliseconds 120
  [Mouse]::mouse_event($up,0,0,0,[UIntPtr]::Zero)
  Start-Sleep 2
}
function Install-App($dir) {
  $installer=(Get-ChildItem "$dir/*-setup.exe").FullName
  $setup=Start-Process $installer -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"/DIR=$dest" -PassThru -Wait
  if ($setup.ExitCode) { throw "Install failed: $($setup.ExitCode)" }
}
function Find-Button {
  $nodes = $bar.FindAll($scope,$all)
  Write-Host ('Taskbar: ' + (($nodes | ForEach-Object {$_.Current.Name}) -join ' | '))
  $matchesForApp=@($nodes | Where-Object { $_.Current.Name -match 'Moosik|moosik' -and $_.Current.BoundingRectangle.Width -gt 0 })
  if ($matchesForApp.Count -gt 1) {throw 'Explorer shows duplicate Moosik buttons'}
  $matchesForApp | Select-Object -First 1
}
$dest=Join-Path $env:RUNNER_TEMP 'MoosikPinned'
Install-App old
$config=Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'refryt\config'
New-Item -Type Directory -Force $config | Out-Null
[IO.File]::WriteAllText((Join-Path $config 'settings.json'),'{"app_icon":"amber"}')
$app=Start-Process "$dest/refryt.exe" -PassThru
Start-Sleep 12
$button=Find-Button
if (!$button) { throw 'Running Moosik button not found' }
Click-Element $button $true
# This is the first pin of a clean test install. Explorer opens the jump list
# with Moosik focused, followed by "Pin to taskbar". Some Windows builds do
# not expose these menu items through UI Automation, but accept menu keys.
$keys=New-Object -ComObject WScript.Shell
$keys.SendKeys('{DOWN}{ENTER}')
Start-Sleep 2
[Mouse]::SetCursorPos(30,30) | Out-Null
Stop-Process -Id $app.Id
Start-Sleep 3
if (!(Find-Button)) { throw 'Moosik not pinned after closing old version' }
$profilePins=Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'Microsoft/Internet Explorer/Quick Launch/User Pinned/TaskBar'
function Show-Pins {
    Get-ChildItem $profilePins -Filter '*.lnk' | ForEach-Object {
        $link=$keys.CreateShortcut($_.FullName)
        Write-Host "$($_.Name): $($link.TargetPath), icon=$($link.IconLocation)"
    }
}
Show-Pins
Install-App dist
$app=Start-Process "$dest/moosik.exe" -PassThru
Start-Sleep 12
$icon=Join-Path $dest 'icons\amber.ico'
Show-Pins
$pin=(Get-ChildItem $profilePins -Filter 'Moosik*.lnk' | Select-Object -First 1).FullName
if (!$pin -or $keys.CreateShortcut($pin).IconLocation -ne "$icon,0") {throw 'Legacy pin was not migrated'}
$app.Refresh()
& "$PSScriptRoot/windows-taskbar.ps1" -Window $app.MainWindowHandle -Exe (Join-Path $dest 'moosik.exe') -Icon $icon -Pin $pin
if (!(Find-Button)) {throw 'No running Moosik taskbar button'}
python scripts/tests/windows-taskbar-image.py $icon taskbar-running.png
$running=$LASTEXITCODE
Stop-Process -Id $app.Id
Start-Sleep 4
python scripts/tests/windows-taskbar-image.py $icon taskbar-closed.png
$closed=$LASTEXITCODE
if ($running -or $closed) { throw "Amber on real upgraded pin: open=$running, closed=$closed (0=pass)" }
# A later selection must also persist on the same real pin. Load the other
# setting through the regular startup path and compare both shell states again.
[IO.File]::WriteAllText((Join-Path $config 'settings.json'),'{"app_icon":"frost"}')
$app=Start-Process "$dest/moosik.exe" -PassThru
Start-Sleep 10
$icon=Join-Path $dest 'icons\frost.ico'
if (!(Find-Button)) {throw 'Moosik taskbar button missing after relaunch'}
python scripts/tests/windows-taskbar-image.py $icon taskbar-frost-running.png
if ($LASTEXITCODE) {throw 'Frost not shown on the running pin'}
Stop-Process -Id $app.Id
Start-Sleep 3
python scripts/tests/windows-taskbar-image.py $icon taskbar-frost-closed.png
if ($LASTEXITCODE) {throw 'Frost not shown on the closed pin'}
