using System.Text.Json;
using Marknexia.Core;

namespace Marknexia.Infrastructure;

public sealed class AppSettings
{
    public AppTheme Theme { get; set; } = AppTheme.System;
    public bool EnforceRepositorySandbox { get; set; } = true;
    public bool AllowExternalLinks { get; set; } = true;
    public bool AllowRemoteAssets { get; set; } = false;
    public bool IsSidebarOpen { get; set; } = true;
    public int SidebarMode { get; set; }
    public string? RepositoryRoot { get; set; }
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
                    _current = Normalize(loaded);
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
        _current = Normalize(settings ?? new AppSettings());
        string? temporaryPath = null;
        try
        {
            string json = JsonSerializer.Serialize(_current, new JsonSerializerOptions { WriteIndented = true });
            string? directory = Path.GetDirectoryName(_settingsFilePath);
            if (!string.IsNullOrEmpty(directory)) Directory.CreateDirectory(directory);

            temporaryPath = $"{_settingsFilePath}.{Guid.NewGuid():N}.tmp";
            File.WriteAllText(temporaryPath, json);
            File.Move(temporaryPath, _settingsFilePath, overwrite: true);
        }
        catch
        {
            // Fail-soft on save error
        }
        finally
        {
            if (temporaryPath != null)
            {
                try { if (File.Exists(temporaryPath)) File.Delete(temporaryPath); }
                catch { }
            }
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

    private static AppSettings Normalize(AppSettings settings)
    {
        if (!Enum.IsDefined(settings.Theme)) settings.Theme = AppTheme.System;
        settings.RepositoryRoot = string.IsNullOrWhiteSpace(settings.RepositoryRoot)
            ? null
            : settings.RepositoryRoot.Trim();
        settings.RecentFiles = NormalizePaths(settings.RecentFiles, 15);
        settings.RecentFolders = NormalizePaths(settings.RecentFolders, 10);
        return settings;
    }

    private static List<string> NormalizePaths(IEnumerable<string>? paths, int limit)
    {
        return (paths ?? Enumerable.Empty<string>())
            .Where(path => !string.IsNullOrWhiteSpace(path))
            .Select(path => path.Trim())
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .Take(limit)
            .ToList();
    }
}
