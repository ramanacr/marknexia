using System.Text.Json;
using Marknexia.Core;

namespace Marknexia.Infrastructure;

public sealed class AppSettings
{
    public AppTheme Theme { get; set; } = AppTheme.System;
    public bool EnforceRepositorySandbox { get; set; } = true;
    public bool AllowExternalLinks { get; set; } = true;
    public bool AllowRemoteAssets { get; set; } = false;
    public List<string> RecentFiles { get; set; } = new();
    public List<string> RecentFolders { get; set; } = new();
}

public sealed class SettingsService
{
    private readonly string _settingsFilePath;
    private AppSettings _current;

    public AppSettings Current => _current;

    public SettingsService(string? customPath = null)
    {
        string appData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        string folder = Path.Combine(appData, "Marknexia");
        Directory.CreateDirectory(folder);
        _settingsFilePath = customPath ?? Path.Combine(folder, "settings.json");

        _current = Load();
    }

    public AppSettings Load()
    {
        try
        {
            if (File.Exists(_settingsFilePath))
            {
                string json = File.ReadAllText(_settingsFilePath);
                var loaded = JsonSerializer.Deserialize<AppSettings>(json);
                if (loaded != null)
                {
                    _current = loaded;
                    return _current;
                }
            }
        }
        catch
        {
            // Fallback to default on read error
        }

        _current = new AppSettings();
        return _current;
    }

    public void Save(AppSettings settings)
    {
        _current = settings ?? new AppSettings();
        try
        {
            string json = JsonSerializer.Serialize(_current, new JsonSerializerOptions { WriteIndented = true });
            File.WriteAllText(_settingsFilePath, json);
        }
        catch
        {
            // Fail-soft on save error
        }
    }

    public void AddRecentFile(string filePath)
    {
        if (string.IsNullOrWhiteSpace(filePath)) return;
        _current.RecentFiles.RemoveAll(f => f.Equals(filePath, StringComparison.OrdinalIgnoreCase));
        _current.RecentFiles.Insert(0, filePath);
        if (_current.RecentFiles.Count > 15)
        {
            _current.RecentFiles = _current.RecentFiles.Take(15).ToList();
        }
        Save(_current);
    }

    public void AddRecentFolder(string folderPath)
    {
        if (string.IsNullOrWhiteSpace(folderPath)) return;
        _current.RecentFolders.RemoveAll(f => f.Equals(folderPath, StringComparison.OrdinalIgnoreCase));
        _current.RecentFolders.Insert(0, folderPath);
        if (_current.RecentFolders.Count > 10)
        {
            _current.RecentFolders = _current.RecentFolders.Take(10).ToList();
        }
        Save(_current);
    }
}
