# Use the actual old installer's shortcut as the migration fixture. Explorer's
# pin registration and its open/closed artwork have a separate interactive test.
$ErrorActionPreference = 'Stop'
$dest = Join-Path $env:RUNNER_TEMP 'MoosikShortcutUpgrade'
function Install-Version($folder) {
    $installer = (Get-Item "$folder/*-setup.exe").FullName
    $setup = Start-Process $installer -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"/DIR=$dest" -Wait -PassThru
    if ($setup.ExitCode) { throw "$folder installer failed: $($setup.ExitCode)" }
}
Install-Version old
$roaming = [Environment]::GetFolderPath('ApplicationData')
$config = Join-Path $roaming 'refryt\config'
$pins = Join-Path $roaming 'Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar'
New-Item -ItemType Directory -Force -Path $config,$pins | Out-Null
[IO.File]::WriteAllText((Join-Path $config 'settings.json'), '{"app_icon":"amber"}')
$pin = Join-Path $pins 'Moosik upgrade fixture.lnk'
Copy-Item (Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Moosik\Moosik.lnk') $pin
$shell = New-Object -ComObject WScript.Shell
if ($shell.CreateShortcut($pin).TargetPath -ne (Join-Path $dest 'refryt.exe')) {
    throw 'Fixture must target the executable installed by version 0.1.20'
}

Install-Version dist
$exe = Join-Path $dest 'moosik.exe'
$icon = Join-Path $dest 'icons\amber.ico'
$app = Start-Process $exe -PassThru
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    do {
        Start-Sleep -Milliseconds 500
        $app.Refresh()
        if ($app.HasExited) { throw "Upgraded player exited: $($app.ExitCode)" }
        $link = $shell.CreateShortcut($pin)
        $ready = $app.MainWindowHandle -ne 0 -and $link.TargetPath -eq $exe -and $link.IconLocation -eq "$icon,0"
    } until ($ready -or [DateTime]::UtcNow -ge $deadline)
    if (!$ready) { throw "Shortcut after upgrade: target=$($link.TargetPath), icon=$($link.IconLocation), window=$($app.MainWindowHandle)" }
    & "$PSScriptRoot/windows-taskbar.ps1" -Window $app.MainWindowHandle -Exe $exe -Icon $icon -Pin $pin
} finally {
    if (!$app.HasExited) { Stop-Process -Id $app.Id }
}
