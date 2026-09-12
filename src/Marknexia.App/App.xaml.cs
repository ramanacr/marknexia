using System;
using System.IO;
using System.Linq;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Diagnostics;
using Marknexia.Core;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using Windows.Storage;

namespace Marknexia.App;

public partial class App : Application
{
    private MainWindow? _window;

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

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        try
        {
            var mainInstance = AppInstance.FindOrRegisterForKey("Marknexia.SingleInstance");
            if (!mainInstance.IsCurrent)
            {
                var activatedArgs = AppInstance.GetCurrent().GetActivatedEventArgs();
                await mainInstance.RedirectActivationToAsync(activatedArgs);
                Process.GetCurrentProcess().Kill();
                return;
            }

            mainInstance.Activated += MainInstance_Activated;

            _window = new MainWindow(ResolveStartupFilePath(args));
            _window.Activate();
        }
        catch (Exception ex)
        {
            LogCrash("OnLaunched", ex);
            throw;
        }
    }

    private void MainInstance_Activated(object? sender, AppActivationArguments e)
    {
        if (_window is null) return;

        _window.DispatcherQueue.TryEnqueue(async () =>
        {
            try
            {
                _window.BringToFront();
                string? filePath = ResolveFilePathFromActivation(e);
                if (!string.IsNullOrWhiteSpace(filePath))
                {
                    await _window.OpenDocumentInTabAsync(filePath);
                }
            }
            catch (Exception ex)
            {
                LogCrash("MainInstance_Activated", ex);
            }
        });
    }

    private static string? ResolveFilePathFromActivation(AppActivationArguments? activation)
    {
        var candidates = new List<string?>();
        if (activation != null)
        {
            ExtractCandidatesFromActivation(activation, candidates);
        }
        candidates.AddRange(Environment.GetCommandLineArgs().Skip(1));
        return StartupFileResolver.FindFirstExisting(candidates);
    }

    private static void ExtractCandidatesFromActivation(AppActivationArguments activation, List<string?> candidates)
    {
        try
        {
            if (activation.Kind == ExtendedActivationKind.File
                && activation.Data is Windows.ApplicationModel.Activation.IFileActivatedEventArgs fileActivation)
            {
                foreach (var file in fileActivation.Files)
                {
                    if (!string.IsNullOrWhiteSpace(file?.Path))
                    {
                        candidates.Add(file.Path);
                    }
                }
            }
            else if (activation.Kind == ExtendedActivationKind.Launch
                && activation.Data is Windows.ApplicationModel.Activation.ILaunchActivatedEventArgs launchData
                && !string.IsNullOrWhiteSpace(launchData.Arguments))
            {
                candidates.Add(launchData.Arguments);
            }
        }
        catch (Exception ex) when (ex is InvalidOperationException or InvalidCastException or COMException)
        {
            LogCrash("ExtractCandidatesFromActivation", ex);
        }
    }

    private static string? ResolveStartupFilePath(LaunchActivatedEventArgs launchArgs)
    {
        var candidates = new List<string?>();

        try
        {
            AppActivationArguments? activation = AppInstance.GetCurrent().GetActivatedEventArgs();
            if (activation != null)
            {
                ExtractCandidatesFromActivation(activation, candidates);
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
