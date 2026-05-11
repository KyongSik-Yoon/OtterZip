using System;
using System.IO;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace OtterZip.App.UserControls;

/// <summary>
/// Inline replacement for the modal <c>ExtractDialog</c>. Hosted in
/// <c>MainWindow</c>'s body area; visibility flipped by
/// <c>MainWindow.SwitchView</c> on demand. Signals the host via
/// <see cref="Submitted"/> / <see cref="Dismissed"/> events instead of
/// returning from <c>ContentDialog.ShowAsync</c>, so the host stays in
/// charge of the view stack.
/// </summary>
public sealed partial class ExtractPanel : UserControl
{
    public event EventHandler<ExtractPanelSubmitArgs>? Submitted;
    public event EventHandler? Dismissed;

    /// <summary>
    /// Raised when the user types into the password field. The host can
    /// hide any prior wrong-password error eagerly so the message
    /// doesn't linger past the user's correction.
    /// </summary>
    public event EventHandler? PasswordEdited;

    public ExtractPanel()
    {
        InitializeComponent();
    }

    /// <summary>
    /// Reset the panel for a new archive. <paramref name="needsPassword"/>
    /// surfaces the password row. <paramref name="suggestedDestination"/>
    /// is the path the host wants pre-filled (typically derived from
    /// Settings_ExtractLocation rules).
    /// </summary>
    public void Configure(string archivePath, string suggestedDestination, bool needsPassword)
    {
        ArchiveNameText.Text = Path.GetFileName(archivePath);
        DestinationField.Text = suggestedDestination;
        PasswordField.Password = "";
        PasswordRow.Visibility = needsPassword ? Visibility.Visible : Visibility.Collapsed;
        ClearError();
        // Focus the password field if encrypted (user likely just wants
        // to type the password and hit Enter); otherwise focus the
        // destination input so they can tweak it without mouse-hunting.
        if (needsPassword)
        {
            _ = PasswordField.Focus(FocusState.Programmatic);
        }
        else
        {
            _ = DestinationField.Focus(FocusState.Programmatic);
        }
    }

    /// <summary>
    /// Show a red error message under the input rows. Used by the host
    /// after a wrong-password retry — the panel stays open so the user
    /// just edits the field and re-submits, no second dialog.
    /// </summary>
    public void ShowError(string message)
    {
        ErrorText.Text = message;
        ErrorText.Visibility = Visibility.Visible;
        // After an error we want the password field active again.
        if (PasswordRow.Visibility == Visibility.Visible)
        {
            PasswordField.SelectAll();
            _ = PasswordField.Focus(FocusState.Programmatic);
        }
    }

    public void ClearError()
    {
        ErrorText.Text = string.Empty;
        ErrorText.Visibility = Visibility.Collapsed;
    }

    private void OnExtractClick(object sender, RoutedEventArgs e)
    {
        var dest = DestinationField.Text;
        if (string.IsNullOrWhiteSpace(dest))
        {
            // Empty destination is undefined — degrade to "여기에 풀기"
            // semantics rather than firing with a blank path.
            OnExtractHereClick(sender, e);
            return;
        }
        Submitted?.Invoke(this, new ExtractPanelSubmitArgs(
            dest,
            PasswordRow.Visibility == Visibility.Visible ? PasswordField.Password : null,
            ExtractHere: false));
    }

    private void OnExtractHereClick(object sender, RoutedEventArgs e)
    {
        Submitted?.Invoke(this, new ExtractPanelSubmitArgs(
            Destination: string.Empty,   // host interprets empty as "next to source"
            Password: PasswordRow.Visibility == Visibility.Visible ? PasswordField.Password : null,
            ExtractHere: true));
    }

    private void OnCancelClick(object sender, RoutedEventArgs e)
        => Dismissed?.Invoke(this, EventArgs.Empty);

    private void OnCloseClick(object sender, RoutedEventArgs e)
        => Dismissed?.Invoke(this, EventArgs.Empty);

    private void OnPasswordFieldChanged(object sender, RoutedEventArgs e)
        => PasswordEdited?.Invoke(this, EventArgs.Empty);

    private async void OnBrowseClick(object sender, RoutedEventArgs e)
    {
        try
        {
            var picker = new Windows.Storage.Pickers.FolderPicker();
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(App.HostWindow);
            WinRT.Interop.InitializeWithWindow.Initialize(picker, hwnd);
            picker.FileTypeFilter.Add("*");
            picker.SuggestedStartLocation = Windows.Storage.Pickers.PickerLocationId.Desktop;
            var folder = await picker.PickSingleFolderAsync();
            if (folder is not null)
            {
                DestinationField.Text = folder.Path;
            }
        }
        catch (Exception)
        {
            // Picker init can fail in unpackaged builds; degrade gracefully.
        }
    }
}
