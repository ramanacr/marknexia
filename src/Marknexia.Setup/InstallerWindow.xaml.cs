using System;
using System.IO;
using System.Threading.Tasks;
using System.Windows;
using Microsoft.Win32;

namespace Marknexia.Setup;

public partial class InstallerWindow : Window
{
    private bool _isComplete;

    public InstallerWindow()
    {
        InitializeComponent();
        TargetDirectoryBox.Text = Program.DefaultInstallDirectory;
        Loaded += (_, _) => TargetDirectoryBox.Focus();
    }

    private void BrowseFolder_Click(object sender, RoutedEventArgs e)
    {
        var dialog = new OpenFolderDialog
        {
            Title = "Select Marknexia installation folder",
            InitialDirectory = TargetDirectoryBox.Text
        };

        if (dialog.ShowDialog() == true)
            TargetDirectoryBox.Text = dialog.FolderName;
    }

    private async void ActionNextButton_Click(object sender, RoutedEventArgs e)
    {
        if (_isComplete)
        {
            Close();
            return;
        }

        string targetDirectory = TargetDirectoryBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(targetDirectory))
        {
            MessageBox.Show("Choose a valid installation folder.", "Marknexia Setup", MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        ConfigStepPanel.Visibility = Visibility.Collapsed;
        ProgressStepPanel.Visibility = Visibility.Visible;
        ActionNextButton.IsEnabled = false;
        CancelButton.IsEnabled = false;
        ProgressStatusText.Text = "Validating the signed application payload…";

        try
        {
            await Task.Run(() => Program.InstallFromWizard(Path.GetFullPath(targetDirectory)));
            ProgressStepPanel.Visibility = Visibility.Collapsed;
            CompleteStepPanel.Visibility = Visibility.Visible;
            ActionNextButton.Content = "Finish";
            ActionNextButton.IsEnabled = true;
            _isComplete = true;
        }
        catch (Exception ex)
        {
            MessageBox.Show($"Marknexia could not be installed.\n\n{ex.Message}", "Marknexia Setup", MessageBoxButton.OK, MessageBoxImage.Error);
            ConfigStepPanel.Visibility = Visibility.Visible;
            ProgressStepPanel.Visibility = Visibility.Collapsed;
            ActionNextButton.IsEnabled = true;
            CancelButton.IsEnabled = true;
        }
    }

    private void CancelButton_Click(object sender, RoutedEventArgs e) => Close();
}
