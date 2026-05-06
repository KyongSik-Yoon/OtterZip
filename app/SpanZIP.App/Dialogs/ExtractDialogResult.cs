namespace SpanZIP.App.Dialogs;

/// <summary>Outcome of <see cref="ExtractDialog"/>.</summary>
public enum ExtractDialogResult
{
    Cancel,
    /// <summary>Use the path the user typed / picked in the dialog.</summary>
    UseCustomPath,
    /// <summary>Extract to a sibling folder named after the archive stem.</summary>
    ExtractHere,
}
