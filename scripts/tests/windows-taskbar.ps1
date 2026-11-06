param([IntPtr]$Window, [string]$Exe, [string]$Icon, [string]$Pin, [System.Diagnostics.Process]$AppProcess)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class TaskbarProps {
    [StructLayout(LayoutKind.Sequential)]
    public struct Key { public Guid fmtid; public uint pid; }
    [StructLayout(LayoutKind.Explicit, Size=24)]
    public struct Value { [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr text; }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface Store {
        void GetCount(out uint count);
        void GetAt(uint i, out Key key);
        void GetValue(ref Key key, out Value value);
    }
    [DllImport("shell32.dll", PreserveSig=false)]
    static extern void SHGetPropertyStoreForWindow(IntPtr hwnd, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out Store store);
    [DllImport("ole32.dll")] static extern int PropVariantClear(ref Value value);
    public static string Read(IntPtr window, uint id) {
        var iid = typeof(Store).GUID;
        Store store;
        SHGetPropertyStoreForWindow(window, ref iid, out store);
        try {
            var key = new Key { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = id };
            Value value;
            store.GetValue(ref key, out value);
            try { return value.vt == 31 ? Marshal.PtrToStringUni(value.text) : null; }
            finally { PropVariantClear(ref value); }
        } finally { Marshal.ReleaseComObject(store); }
    }
}
'@
$expected = @{ 2 = ('"' + $Exe + '"'); 3 = "$Icon,0"; 4 = 'Moosik'; 5 = 'Moosik.Desktop' }
# Startup can briefly expose an HWND before Explorer sees its taskbar properties.
# Refresh the process handle and wait for the complete metadata on the same window.
$deadline = [DateTime]::UtcNow.AddSeconds(30)
do {
    if ($AppProcess) {
        $AppProcess.Refresh()
        if ($AppProcess.HasExited) { throw 'Moosik exited while waiting for its taskbar window' }
        $Window = $AppProcess.MainWindowHandle
    }
    $mismatch = 'No main window yet'
    if ($Window -ne [IntPtr]::Zero) {
        $mismatch = $null
        foreach ($key in $expected.Keys) {
            $actual = [TaskbarProps]::Read($Window, $key)
            if ($actual -ne $expected[$key]) {
                $mismatch = "Taskbar property $key was '$actual', expected '$($expected[$key])'"
                break
            }
        }
    }
    if (!$mismatch) { break }
    Start-Sleep -Milliseconds 300
} while ([DateTime]::UtcNow -lt $deadline)
if ($mismatch) { throw $mismatch }
$shell = New-Object -ComObject Shell.Application
$shortcuts = @((Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Moosik\Moosik.lnk'))
$launcherIcon = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Moosik\Taskbar\current.ico'
$launcher = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcuts[0])
$location = [Environment]::ExpandEnvironmentVariables($launcher.IconLocation)
if ($location -ne "$launcherIcon,0" -or (Get-FileHash $launcherIcon).Hash -ne (Get-FileHash $Icon).Hash) {
    throw "The installed launcher does not point to the selected per-user artwork: $location"
}
if ($Pin) { $shortcuts += $Pin }
foreach ($shortcut in $shortcuts) {
    $folder = $shell.Namespace((Split-Path $shortcut))
    $item = $folder.ParseName((Split-Path -Leaf $shortcut))
    if ($null -eq $item -or $item.ExtendedProperty('System.AppUserModel.ID') -ne 'Moosik.Desktop') {
        throw "Wrong taskbar identity on $shortcut"
    }
}
