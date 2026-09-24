param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath
)

$ErrorActionPreference = 'Stop'
$resolved = (Resolve-Path -LiteralPath $ExePath).Path
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class RustShellWindowProbe {
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")]
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    private delegate bool EnumWindowsProc(IntPtr window, IntPtr data);
    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr data);
    [DllImport("user32.dll")]
    private static extern bool EnumChildWindows(IntPtr parent, EnumWindowsProc callback, IntPtr data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetClassName(IntPtr window, StringBuilder name, int capacity);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, StringBuilder title, int capacity);
    public static IntPtr FindVisibleWindowForProcess(uint processId, string expectedClass, string expectedTitle) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, _) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != processId || !IsWindowVisible(window)) return true;
            var name = new StringBuilder(256);
            var title = new StringBuilder(256);
            GetClassName(window, name, name.Capacity);
            GetWindowText(window, title, title.Capacity);
            if (name.ToString() != expectedClass || title.ToString() != expectedTitle) return true;
            found = window;
            return false;
        }, IntPtr.Zero);
        return found;
    }
    public static string DescribeWindowsForProcess(uint processId) {
        var descriptions = new StringBuilder();
        EnumWindows((window, _) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == processId) {
                var name = new StringBuilder(256);
                var title = new StringBuilder(256);
                GetClassName(window, name, name.Capacity);
                GetWindowText(window, title, title.Capacity);
                descriptions.Append($"[HWND={window}, class={name}, title={title}, visible={IsWindowVisible(window)}]");
            }
            return true;
        }, IntPtr.Zero);
        return descriptions.ToString();
    }
    public static bool HasChildText(IntPtr parent, string expected) {
        bool found = false;
        EnumChildWindows(parent, (child, _) => {
            var title = new StringBuilder(1024);
            GetWindowText(child, title, title.Capacity);
            if (title.ToString().Contains(expected, StringComparison.Ordinal)) {
                found = true;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
$process = Start-Process -FilePath $resolved -PassThru
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    $visible = $false
    $window = [IntPtr]::Zero
    while ([DateTime]::UtcNow -lt $deadline) {
        $process.Refresh()
        if ($process.HasExited) {
            throw "Rust shell exited before opening a window (exit $($process.ExitCode))."
        }
        $window = [RustShellWindowProbe]::FindVisibleWindowForProcess(
            [uint32]$process.Id, 'MarknexiaRustPreviewWindow', 'Marknexia Rust Preview')
        [uint32]$owner = 0
        if ($window -ne [IntPtr]::Zero -and
            [RustShellWindowProbe]::GetWindowThreadProcessId($window, [ref]$owner) -ne 0 -and
            $owner -eq $process.Id -and
            [RustShellWindowProbe]::IsWindowVisible($window)) {
            $visible = $true
            break
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $visible) {
        $process.Refresh()
        $windowVisible = if ($window -ne [IntPtr]::Zero) { [RustShellWindowProbe]::IsWindowVisible($window) } else { $false }
        $windows = [RustShellWindowProbe]::DescribeWindowsForProcess([uint32]$process.Id)
        throw "Rust shell window not verified within 10 seconds (PID $($process.Id), found HWND $window, owner $owner, visible $windowVisible, process MainWindowHandle $($process.MainWindowHandle), title '$($process.MainWindowTitle)', windows $windows)."
    }
    if (-not [RustShellWindowProbe]::HasChildText($window, 'Document viewport not connected')) {
        throw 'Rust shell preview did not expose its native development-status surface.'
    }
    Write-Output "Rust shell visible: PID $($process.Id), HWND $window."
}
finally {
    $process.Refresh()
    if (-not $process.HasExited) {
        if ($window -ne [IntPtr]::Zero) {
            [uint32]$closeOwner = 0
            if ([RustShellWindowProbe]::GetWindowThreadProcessId($window, [ref]$closeOwner) -ne 0 -and
                $closeOwner -eq $process.Id) {
                [void][RustShellWindowProbe]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
            }
        }
        if (-not $process.WaitForExit(2000)) {
            Stop-Process -Id $process.Id -Force
        }
    }
    $process.Dispose()
}
