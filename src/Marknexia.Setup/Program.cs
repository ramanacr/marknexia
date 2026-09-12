using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32;

namespace Marknexia.Setup;

internal static class Program
{
    private const string ProductName = "Marknexia";
    private const string InstallerFileName = "MarknexiaSetup.exe";
    private const string PayloadResourceName = "Marknexia.Setup.Payload.zip";
    private const long MaxPayloadBytes = 512L * 1024 * 1024;
    private const string UninstallRegistryRoot = @"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    private const string ClassesRegistryRoot = @"Software\Classes";
    private const string PreviewHandlerAssociation = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
    private const string WindowsTextPreviewHandler = "{1531d583-8375-4d3f-b5fb-d23bbd169f22}";

    private static readonly string[] RequiredPayloadFiles =
    [
        "Marknexia.App.exe",
        "Marknexia.App.dll",
        "Marknexia.App.pri",
        "Assets/markdown-file.ico",
        "Assets/MarkdownFileLogo.png",
        "marknexia-sbom.spdx.json"
    ];

    [STAThread]
    public static int Main(string[] args)
    {
        if (args.Length == 0)
            return RunInteractive();

        InstallerOptions? options = null;
        try
        {
            options = InstallerOptions.Parse(args);
            return options.Mode switch
            {
                InstallerMode.Verify => Verify(options),
                InstallerMode.Install => Install(options),
                InstallerMode.Uninstall => Uninstall(options),
                _ => throw new InvalidOperationException("Unknown installer mode.")
            };
        }
        catch (Exception ex) when (ex is not StackOverflowException and not OutOfMemoryException)
        {
            Console.Error.WriteLine($"{ProductName} Setup failed: {ex.Message}");
            if (options is not { Silent: true })
                ShowMessage(ex.Message, $"{ProductName} Setup", MessageBoxType.Error);
            return 1;
        }
    }

    internal static string DefaultInstallDirectory => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Programs",
        ProductName);

    internal static int InstallFromWizard(string directory)
    {
        EnsureArchitecture(CurrentArchitectureToken);
        var options = new InstallerOptions(
            InstallerMode.Install,
            Silent: true,
            Directory: directory,
            ExpectedArchitecture: CurrentArchitectureToken,
            Scope: "Production");
        return Install(options);
    }

    private static int RunInteractive()
    {
        var application = new System.Windows.Application
        {
            ShutdownMode = System.Windows.ShutdownMode.OnMainWindowClose
        };
        return application.Run(new InstallerWindow());
    }

    private static int Verify(InstallerOptions options)
    {
        EnsureArchitecture(options.ExpectedArchitecture);
        using ZipArchive archive = OpenPayload();
        ValidateArchive(archive);
        Console.WriteLine($"Installer payload verified for {CurrentArchitectureToken}.");
        return 0;
    }

    private static int Install(InstallerOptions options)
    {
        EnsureArchitecture(options.ExpectedArchitecture);
        string installDirectory = GetInstallDirectory(options);
        EnsureInstallDirectoryIsSafe(installDirectory);
        EnsureApplicationIsNotRunning(options.Silent);
        using (ZipArchive archive = OpenPayload())
        {
            ValidateArchive(archive);
            string stagingDirectory = installDirectory + ".staging-" + Guid.NewGuid().ToString("N");
            string? backupDirectory = null;
            try
            {
                Directory.CreateDirectory(stagingDirectory);
                ExtractArchive(archive, stagingDirectory);
                CopyRunningInstaller(stagingDirectory);

                if (Directory.Exists(installDirectory))
                {
                    backupDirectory = installDirectory + ".backup-" + Guid.NewGuid().ToString("N");
                    Directory.Move(installDirectory, backupDirectory);
                }

                Directory.Move(stagingDirectory, installDirectory);
                RegisterShellIntegration(options, installDirectory);
                if (backupDirectory != null) TryDeleteDirectory(backupDirectory);
                ShowMessageIfInteractive(options, $"{ProductName} was installed successfully. It is available from the Start menu.", MessageBoxType.Information);
                return 0;
            }
            catch
            {
                TryDeleteDirectory(stagingDirectory);
                if (Directory.Exists(installDirectory)) TryDeleteDirectory(installDirectory);
                if (backupDirectory != null && Directory.Exists(backupDirectory))
                    Directory.Move(backupDirectory, installDirectory);
                throw;
            }
        }
    }

    private static int Uninstall(InstallerOptions options)
    {
        string installDirectory = GetInstallDirectoryFromRegistry(options) ?? GetInstallDirectory(options);
        EnsureInstallDirectoryIsSafe(installDirectory);
        UnregisterShellIntegration(options, installDirectory);
        if (IsRunningFromDirectory(installDirectory))
        {
            ScheduleSelfDelete(installDirectory);
        }
        else
        {
            TryDeleteDirectory(installDirectory);
        }

        ShowMessageIfInteractive(options, $"{ProductName} was uninstalled.", MessageBoxType.Information);
        return 0;
    }

    private static void EnsureApplicationIsNotRunning(bool silent)
    {
        if (!Process.GetProcessesByName("Marknexia.App").Any()) return;
        string message = "Close Marknexia before installing or upgrading it.";
        if (silent) throw new InvalidOperationException(message);
        ShowMessage(message, $"{ProductName} Setup", MessageBoxType.Warning);
        throw new InvalidOperationException(message);
    }

    private static void RegisterShellIntegration(InstallerOptions options, string installDirectory)
    {
        string executable = Path.Combine(installDirectory, "Marknexia.App.exe");
        string programId = GetProgramId(options);
        string fileArgument = Quote("%1");
        string markdownIcon = Path.Combine(installDirectory, "Assets", "markdown-file.ico");
        string[] markdownExtensions = [".md", ".markdown", ".mdown", ".mkdn"];

        using (RegistryKey classes = Registry.CurrentUser.CreateSubKey(ClassesRegistryRoot) ?? throw new InvalidOperationException("Could not open per-user shell registration."))
        {
            using (RegistryKey progId = classes.CreateSubKey(programId))
            {
                progId.SetValue(null, "Marknexia Markdown Document");
                progId.SetValue("FriendlyTypeName", "Marknexia Markdown Document");
                using (RegistryKey progShell = progId.CreateSubKey("shell"))
                {
                    progShell.SetValue(null, "open");
                }
                using (RegistryKey icon = progId.CreateSubKey("DefaultIcon"))
                {
                    icon.SetValue(null, $"{markdownIcon},0");
                }
                using (RegistryKey preview = progId.CreateSubKey($"ShellEx\\{PreviewHandlerAssociation}"))
                {
                    preview.SetValue(null, WindowsTextPreviewHandler);
                }
                using (RegistryKey command = progId.CreateSubKey(@"shell\open\command"))
                {
                    command.SetValue(null, $"{Quote(executable)} {fileArgument}");
                }
            }

            // Register executable application capabilities for Open With and Default Programs
            using (RegistryKey appKey = classes.CreateSubKey(@"Applications\Marknexia.App.exe"))
            {
                appKey.SetValue("FriendlyAppName", ProductName);
                using (RegistryKey appIcon = appKey.CreateSubKey("DefaultIcon"))
                {
                    appIcon.SetValue(null, $"{markdownIcon},0");
                }
                using (RegistryKey appCommand = appKey.CreateSubKey(@"shell\open\command"))
                {
                    appCommand.SetValue(null, $"{Quote(executable)} {fileArgument}");
                }
                using (RegistryKey supportedTypes = appKey.CreateSubKey("SupportedTypes"))
                {
                    foreach (string extension in markdownExtensions)
                    {
                        supportedTypes.SetValue(extension, string.Empty);
                    }
                }
            }

            foreach (string extension in markdownExtensions)
            {
                using RegistryKey extensionKey = classes.CreateSubKey(extension);
                extensionKey.SetValue(null, programId);
                extensionKey.SetValue("PerceivedType", "text");
                using (RegistryKey extensionPreview = extensionKey.CreateSubKey($"ShellEx\\{PreviewHandlerAssociation}"))
                {
                    extensionPreview.SetValue(null, WindowsTextPreviewHandler);
                }
                using (RegistryKey openWithProgids = extensionKey.CreateSubKey("OpenWithProgids"))
                {
                    openWithProgids.SetValue(programId, Array.Empty<byte>(), RegistryValueKind.None);
                }
                using (RegistryKey openWithList = extensionKey.CreateSubKey(@"OpenWithList\Marknexia.App.exe"))
                {
                }
            }
        }

        string startMenuDirectory = GetStartMenuDirectory(options);
        Directory.CreateDirectory(startMenuDirectory);
        CreateShortcut(
            Path.Combine(startMenuDirectory, "Marknexia.lnk"),
            executable,
            installDirectory,
            "Marknexia Markdown viewer");

        string uninstallKeyName = GetProductKey(options);
        using (RegistryKey uninstall = Registry.CurrentUser.CreateSubKey($"{UninstallRegistryRoot}\\{uninstallKeyName}")
            ?? throw new InvalidOperationException("Could not register the per-user uninstaller."))
        {
            string installedSetup = Path.Combine(installDirectory, InstallerFileName);
            string scopeArgument = options.IsProduction ? string.Empty : $" --scope {Quote(options.Scope)}";
            uninstall.SetValue("DisplayName", ProductName);
            uninstall.SetValue("DisplayVersion", CurrentProductVersion);
            uninstall.SetValue("Publisher", "Marknexia");
            uninstall.SetValue("InstallLocation", installDirectory);
            uninstall.SetValue("DisplayIcon", executable);
            uninstall.SetValue("UninstallString", $"{Quote(installedSetup)} --uninstall --dir {Quote(installDirectory)}{scopeArgument}");
            uninstall.SetValue("QuietUninstallString", $"{Quote(installedSetup)} --uninstall --silent --dir {Quote(installDirectory)}{scopeArgument}");
            uninstall.SetValue("NoModify", 1, RegistryValueKind.DWord);
            uninstall.SetValue("NoRepair", 1, RegistryValueKind.DWord);
        }

        RefreshShellAssociations();
    }

    private static void UnregisterShellIntegration(InstallerOptions options, string installDirectory)
    {
        string programId = GetProgramId(options);
        string[] markdownExtensions = [".md", ".markdown", ".mdown", ".mkdn"];
        using (RegistryKey? classes = Registry.CurrentUser.OpenSubKey(ClassesRegistryRoot, writable: true))
        {
            if (classes != null)
            {
                foreach (string extension in markdownExtensions)
                {
                    using RegistryKey? extensionKey = classes.OpenSubKey(extension, writable: true);
                    if (extensionKey != null)
                    {
                        if (string.Equals(extensionKey.GetValue(null) as string, programId, StringComparison.Ordinal))
                        {
                            extensionKey.DeleteValue(string.Empty, throwOnMissingValue: false);
                        }
                        using (RegistryKey? openWith = extensionKey.OpenSubKey("OpenWithProgids", writable: true))
                        {
                            openWith?.DeleteValue(programId, throwOnMissingValue: false);
                        }
                        using (RegistryKey? openWithList = extensionKey.OpenSubKey("OpenWithList", writable: true))
                        {
                            openWithList?.DeleteSubKey("Marknexia.App.exe", throwOnMissingSubKey: false);
                        }
                    }
                }
                classes.DeleteSubKeyTree(programId, throwOnMissingSubKey: false);
                classes.DeleteSubKeyTree(@"Applications\Marknexia.App.exe", throwOnMissingSubKey: false);
            }
        }

        string startMenuDirectory = GetStartMenuDirectory(options);
        TryDeleteDirectory(startMenuDirectory);
        using (RegistryKey? uninstallRoot = Registry.CurrentUser.OpenSubKey(UninstallRegistryRoot, writable: true))
            uninstallRoot?.DeleteSubKeyTree(GetProductKey(options), throwOnMissingSubKey: false);

        _ = installDirectory;
        RefreshShellAssociations();
    }

    private static string? GetInstallDirectoryFromRegistry(InstallerOptions options)
    {
        using RegistryKey? uninstallRoot = Registry.CurrentUser.OpenSubKey(UninstallRegistryRoot);
        return uninstallRoot?.OpenSubKey(GetProductKey(options))?.GetValue("InstallLocation") as string;
    }

    private static void ExtractArchive(ZipArchive archive, string destination)
    {
        long totalBytes = 0;
        foreach (ZipArchiveEntry entry in archive.Entries)
        {
            string name = entry.FullName.Replace('\\', '/');
            if (string.IsNullOrWhiteSpace(name)) continue;
            string target = GetSafeArchivePath(destination, name);
            if (name.EndsWith('/'))
            {
                Directory.CreateDirectory(target);
                continue;
            }

            if (entry.Length < 0 || entry.Length > MaxPayloadBytes || totalBytes > MaxPayloadBytes - entry.Length)
                throw new InvalidDataException("The installer payload exceeds the supported size limit.");
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            using Stream source = entry.Open();
            using FileStream output = new(target, FileMode.CreateNew, FileAccess.Write, FileShare.None);
            source.CopyTo(output);
            totalBytes += output.Length;
        }
    }

    private static void ValidateArchive(ZipArchive archive)
    {
        HashSet<string> files = new(StringComparer.OrdinalIgnoreCase);
        foreach (ZipArchiveEntry entry in archive.Entries)
        {
            string name = entry.FullName.Replace('\\', '/');
            if (string.IsNullOrWhiteSpace(name)) continue;
            if (name.StartsWith('/') || name.Contains(':', StringComparison.Ordinal) || name.Split('/').Contains("..", StringComparer.Ordinal))
                throw new InvalidDataException("The installer payload contains an unsafe path.");
            if (!name.EndsWith('/')) files.Add(name);
            if (entry.Length > MaxPayloadBytes) throw new InvalidDataException("The installer payload contains an oversized file.");
        }

        foreach (string requiredFile in RequiredPayloadFiles)
        {
            if (!files.Contains(requiredFile)) throw new InvalidDataException($"The installer payload is missing '{requiredFile}'.");
        }
    }

    private static string GetSafeArchivePath(string destination, string name)
    {
        string target = Path.GetFullPath(Path.Combine(destination, name.Replace('/', Path.DirectorySeparatorChar)));
        string root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(destination));
        if (!target.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("The installer payload contains a path traversal entry.");
        return target;
    }

    private static ZipArchive OpenPayload()
    {
        Stream stream = Assembly.GetExecutingAssembly().GetManifestResourceStream(PayloadResourceName)
            ?? throw new InvalidDataException("This setup executable does not contain an application payload.");
        return new ZipArchive(stream, ZipArchiveMode.Read, leaveOpen: false);
    }

    private static void CopyRunningInstaller(string stagingDirectory)
    {
        string? source = Environment.ProcessPath;
        if (string.IsNullOrWhiteSpace(source) || !File.Exists(source))
            throw new InvalidOperationException("The setup executable path could not be determined.");
        File.Copy(source, Path.Combine(stagingDirectory, InstallerFileName), overwrite: true);
    }

    private static void ScheduleSelfDelete(string installDirectory)
    {
        string literal = installDirectory.Replace("'", "''");
        string script = $"Start-Sleep -Milliseconds 750; if (Test-Path -LiteralPath '{literal}') {{ Remove-Item -LiteralPath '{literal}' -Recurse -Force -ErrorAction SilentlyContinue }}";
        var startInfo = new ProcessStartInfo
        {
            FileName = "powershell.exe",
            UseShellExecute = false,
            CreateNoWindow = true,
            WindowStyle = ProcessWindowStyle.Hidden
        };
        startInfo.ArgumentList.Add("-NoProfile");
        startInfo.ArgumentList.Add("-NonInteractive");
        startInfo.ArgumentList.Add("-WindowStyle");
        startInfo.ArgumentList.Add("Hidden");
        startInfo.ArgumentList.Add("-Command");
        startInfo.ArgumentList.Add(script);
        Process.Start(startInfo)?.Dispose();
    }

    private static void CreateShortcut(string shortcutPath, string targetPath, string workingDirectory, string description)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(shortcutPath)!);
        object? shellLinkObject = null;
        try
        {
            Type shellLinkType = Type.GetTypeFromCLSID(new Guid("00021401-0000-0000-C000-000000000046"), throwOnError: true)
                ?? throw new InvalidOperationException("Windows Shell Link support is unavailable.");
            shellLinkObject = Activator.CreateInstance(shellLinkType)
                ?? throw new InvalidOperationException("Windows Shell Link creation failed.");
            var shellLink = (IShellLinkW)shellLinkObject;
            shellLink.SetPath(targetPath);
            shellLink.SetWorkingDirectory(workingDirectory);
            shellLink.SetDescription(description);
            shellLink.SetIconLocation(targetPath, 0);
            ((IPersistFile)shellLinkObject).Save(shortcutPath, false);
        }
        finally
        {
            if (shellLinkObject != null && Marshal.IsComObject(shellLinkObject))
                Marshal.FinalReleaseComObject(shellLinkObject);
        }
    }

    private static string GetInstallDirectory(InstallerOptions options) =>
        Path.GetFullPath(options.Directory ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Programs",
            ProductName));

    private static void EnsureInstallDirectoryIsSafe(string directory)
    {
        if (string.IsNullOrWhiteSpace(directory) || Path.GetPathRoot(directory)?.Equals(directory, StringComparison.OrdinalIgnoreCase) == true)
            throw new InvalidOperationException("The install directory is not safe.");
    }

    private static string GetProductKey(InstallerOptions options) => options.IsProduction ? ProductName : $"{ProductName}-{options.Scope}";
    private static string GetProgramId(InstallerOptions options) => options.IsProduction ? "Marknexia.MarkdownFile" : $"Marknexia.MarkdownFile.{options.Scope}";
    private static string GetStartMenuDirectory(InstallerOptions options) => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.StartMenu), "Programs", GetProductKey(options));
    private static bool IsRunningFromDirectory(string directory) =>
        Environment.ProcessPath is string processPath
        && Path.GetFullPath(processPath).StartsWith(Path.TrimEndingDirectorySeparator(directory) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    private static string CurrentArchitectureToken => RuntimeInformation.ProcessArchitecture switch
    {
        Architecture.X64 => "x64",
        Architecture.Arm64 => "arm64",
        _ => "unsupported"
    };
    private static string CurrentProductVersion => typeof(Program).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion.Split('+')[0] ?? "0.0.0";

    private static void EnsureArchitecture(string expected)
    {
        if (CurrentArchitectureToken == "unsupported" || !CurrentArchitectureToken.Equals(expected, StringComparison.OrdinalIgnoreCase))
            throw new PlatformNotSupportedException($"This installer is for {expected}; the current executable is {CurrentArchitectureToken}.");
    }

    private static void TryDeleteDirectory(string path)
    {
        if (Directory.Exists(path)) Directory.Delete(path, recursive: true);
    }

    private static string Quote(string value) => $"\"{value.Replace("\"", "\\\"")}\"";

    private static void ShowMessageIfInteractive(InstallerOptions options, string message, MessageBoxType type)
    {
        if (!options.Silent) ShowMessage(message, $"{ProductName} Setup", type);
    }

    private static void ShowMessage(string message, string title, MessageBoxType type) =>
        _ = MessageBox(IntPtr.Zero, message, title, (uint)type | 0x00010000);

    private enum InstallerMode { Install, Uninstall, Verify }
    private enum MessageBoxType { Information = 0x40, Warning = 0x30, Error = 0x10 }

    private sealed record InstallerOptions(
        InstallerMode Mode,
        bool Silent,
        string? Directory,
        string ExpectedArchitecture,
        string Scope)
    {
        public bool IsProduction => Scope.Equals("Production", StringComparison.Ordinal);

        public static InstallerOptions Parse(string[] args)
        {
            InstallerMode mode = InstallerMode.Install;
            bool silent = false;
            string? directory = null;
            string expectedArchitecture = CurrentArchitectureToken;
            string scope = "Production";

            for (int index = 0; index < args.Length; index++)
            {
                string argument = args[index].Trim();
                switch (argument.ToLowerInvariant())
                {
                    case "--install":
                    case "/install": mode = InstallerMode.Install; break;
                    case "--uninstall":
                    case "/uninstall": mode = InstallerMode.Uninstall; break;
                    case "--verify":
                    case "/verify": mode = InstallerMode.Verify; break;
                    case "--silent":
                    case "/silent": silent = true; break;
                    case "--dir":
                    case "/dir": directory = ReadValue(args, ref index, argument); break;
                    case "--architecture": expectedArchitecture = ReadValue(args, ref index, argument).ToLowerInvariant(); break;
                    case "--scope": scope = ValidateScope(ReadValue(args, ref index, argument)); break;
                    case "--help":
                    case "/?":
                        Console.WriteLine("Marknexia Setup: --install, --uninstall, --verify, --silent, --dir <path>, --architecture <x64|arm64>");
                        Environment.Exit(0);
                        break;
                    default: throw new ArgumentException($"Unknown setup option '{argument}'.");
                }
            }

            if (expectedArchitecture is not ("x64" or "arm64"))
                throw new ArgumentException("Architecture must be x64 or arm64.");
            return new InstallerOptions(mode, silent, directory, expectedArchitecture, scope);
        }

        private static string ReadValue(string[] args, ref int index, string option)
        {
            if (++index >= args.Length || string.IsNullOrWhiteSpace(args[index]))
                throw new ArgumentException($"Option {option} requires a value.");
            return args[index];
        }

        private static string ValidateScope(string value)
        {
            if (value.Length is < 1 or > 60 || value.Any(character => !(char.IsLetterOrDigit(character) || character is '-' or '_')))
                throw new ArgumentException("Installer scope contains unsupported characters.");
            return value;
        }
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int MessageBox(IntPtr hWnd, string text, string caption, uint type);

    [DllImport("shell32.dll")]
    private static extern void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);

    private const int ShcneAssocChanged = 0x08000000;
    private const uint ShcnfIdList = 0x0000;

    private static void RefreshShellAssociations()
    {
        try
        {
            SHChangeNotify(ShcneAssocChanged, ShcnfIdList, IntPtr.Zero, IntPtr.Zero);
        }
        catch { }
    }

    [ComImport]
    [Guid("000214F9-0000-0000-C000-000000000046")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        [PreserveSig] int GetPath(StringBuilder file, int maxPath, IntPtr findData, uint flags);
        [PreserveSig] int GetIDList(out IntPtr idList);
        [PreserveSig] int SetIDList(IntPtr idList);
        [PreserveSig] int GetDescription(StringBuilder name, int maxName);
        [PreserveSig] int SetDescription([MarshalAs(UnmanagedType.LPWStr)] string name);
        [PreserveSig] int GetWorkingDirectory(StringBuilder directory, int maxPath);
        [PreserveSig] int SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string directory);
        [PreserveSig] int GetArguments(StringBuilder arguments, int maxPath);
        [PreserveSig] int SetArguments([MarshalAs(UnmanagedType.LPWStr)] string arguments);
        [PreserveSig] int GetHotkey(out short hotkey);
        [PreserveSig] int SetHotkey(short hotkey);
        [PreserveSig] int GetShowCmd(out int showCommand);
        [PreserveSig] int SetShowCmd(int showCommand);
        [PreserveSig] int GetIconLocation(StringBuilder iconPath, int maxPath, out int iconIndex);
        [PreserveSig] int SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string iconPath, int iconIndex);
        [PreserveSig] int SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string path, uint reserved);
        [PreserveSig] int Resolve(IntPtr owner, uint flags);
        [PreserveSig] int SetPath([MarshalAs(UnmanagedType.LPWStr)] string file);
    }

    [ComImport]
    [Guid("0000010B-0000-0000-C000-000000000046")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IPersistFile
    {
        void GetClassID(out Guid classId);
        void IsDirty();
        void Load([MarshalAs(UnmanagedType.LPWStr)] string fileName, uint mode);
        void Save([MarshalAs(UnmanagedType.LPWStr)] string fileName, [MarshalAs(UnmanagedType.Bool)] bool remember);
        void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string fileName);
        void GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string fileName);
    }
}
