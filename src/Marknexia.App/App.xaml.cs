using System;
using System.IO;
using System.Linq;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using Marknexia.Core;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using Windows.Storage;

namespace Marknexia.App;

public partial class App : Application
{
    private Window? _window;

    public App()
    {
        try
        {
            // Opt-in diagnostics retain the original managed stack before the
            // WinRT boundary turns a failure into an HRESULT-only exception.
            if (Environment.GetEnvironmentVariable("MARKNEXIA_TRACE_STARTUP") == "1")
            {
                AppDomain.CurrentDomain.FirstChanceException += (sender, e) =>
                {
                    if (e.Exception is InvalidCastException)
                        LogCrash("FirstChance_InvalidCast", e.Exception);
                };
            }
            UnhandledException += (sender, e) =>
            {
                LogCrash("App_UnhandledException", e.Exception);
            };

            AppDomain.CurrentDomain.UnhandledException += (sender, e) =>
            {
                LogCrash("CurrentDomain_UnhandledException", e.ExceptionObject as Exception);
            };

            InitializeComponent();
        }
        catch (Exception ex)
        {
            LogCrash("App_Constructor", ex);
            throw;
        }
    }

    private static void LogCrash(string tag, Exception? ex)
    {
        try
        {
            string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            string dir = Path.Combine(localAppData, "Marknexia");
            Directory.CreateDirectory(dir);
            string path = Path.Combine(dir, "crash.log");
            File.AppendAllText(path, $"[{DateTime.Now:O}] [{tag}]\n{ex?.ToString()}\n\n");
        }
        catch { }
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        try
        {
            _window = new MainWindow(ResolveStartupFilePath(args));
            _window.Activate();
        }
        catch (Exception ex)
        {
            LogCrash("OnLaunched", ex);
            throw;
        }
    }

    private static string? ResolveStartupFilePath(LaunchActivatedEventArgs launchArgs)
    {
        var candidates = new List<string?>();

        try
        {
            AppActivationArguments? activation = AppInstance.GetCurrent().GetActivatedEventArgs();
            if (activation?.Kind == ExtendedActivationKind.File
                && activation.Data is Windows.ApplicationModel.Activation.IFileActivatedEventArgs fileActivation)
            {
                candidates.AddRange(fileActivation.Files
                    .OfType<StorageFile>()
                    .Select(file => file.Path));
            }
        }
        catch (Exception ex) when (ex is InvalidOperationException or InvalidCastException or COMException)
        {
            LogCrash("ResolveStartupFilePath", ex);
        }

        if (!string.IsNullOrWhiteSpace(launchArgs.Arguments))
        {
            candidates.Add(launchArgs.Arguments);
        }

        candidates.AddRange(Environment.GetCommandLineArgs().Skip(1));
        return StartupFileResolver.FindFirstExisting(candidates);
    }
}
