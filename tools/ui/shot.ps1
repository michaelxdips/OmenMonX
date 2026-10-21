param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$OutFile,
    [int]$WaitSeconds = 8
)
# Capture a window's pixels via PrintWindow.
#
# Windows are found by enumerating the process's visible top-level windows and
# taking the largest. Process.MainWindowHandle is not reliable here: it can point
# at a 14x14 hidden helper window, which produced a 14x14 "screenshot" during
# development and looked like a rendering failure.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

    public static IntPtr FindMain(uint targetPid) {
        IntPtr best = IntPtr.Zero;
        long bestArea = 0;
        EnumWindows((h, l) => {
            uint pid;
            GetWindowThreadProcessId(h, out pid);
            if (pid != targetPid) return true;
            if (!IsWindowVisible(h)) return true;
            RECT r;
            if (!GetWindowRect(h, out r)) return true;
            long area = (long)(r.Right - r.Left) * (r.Bottom - r.Top);
            if (area > bestArea) { bestArea = area; best = h; }
            return true;
        }, IntPtr.Zero);
        return best;
    }
    public static string Title(IntPtr h) {
        int n = GetWindowTextLength(h);
        var sb = new StringBuilder(n + 2);
        GetWindowText(h, sb, sb.Capacity);
        return sb.ToString();
    }
}
"@
$p = Start-Process -FilePath $Exe -PassThru
try {
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    $h = [IntPtr]::Zero
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        if ($p.HasExited) { throw "process exited with $($p.ExitCode)" }
        $h = [Win]::FindMain([uint32]$p.Id)
        if ($h -ne [IntPtr]::Zero) { break }
    }
    if ($h -eq [IntPtr]::Zero) { throw "no visible window" }
    if ([Win]::IsIconic($h)) { [Win]::ShowWindow($h, 9) | Out-Null }
    Start-Sleep -Milliseconds 2500
    $r = New-Object Win+RECT
    [Win]::GetWindowRect($h, [ref]$r) | Out-Null
    $w = $r.Right - $r.Left; $hh = $r.Bottom - $r.Top
    "window: $([Win]::Title($h))  ${w}x${hh}"
    $bmp = New-Object System.Drawing.Bitmap $w, $hh
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    # PW_RENDERFULLCONTENT (2) is required for GPU-composited windows.
    $ok = [Win]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    if (-not $ok) { throw "PrintWindow failed" }
    $bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    "captured -> $OutFile"
}
finally {
    if (-not $p.HasExited) { $p.CloseMainWindow() | Out-Null; Start-Sleep -Milliseconds 900 }
    if (-not $p.HasExited) { $p.Kill() }
}
