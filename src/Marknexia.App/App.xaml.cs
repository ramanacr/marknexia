using System;
using System.IO;
using Microsoft.UI.Xaml;

namespace Marknexia.App;

public partial class App : Application
{
    private Window? _window;

    public App()
    {
        try
        {
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
            _window = new MainWindow();
            _window.Activate();
        }
        catch (Exception ex)
        {
            LogCrash("OnLaunched", ex);
            throw;
        }
    }
}
