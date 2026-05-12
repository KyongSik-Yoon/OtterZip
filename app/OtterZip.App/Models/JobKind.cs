namespace OtterZip.App.Models;

/// <summary>
/// What the job is doing. Drives the icon, label, and which delegate the
/// JobQueue runs.
/// </summary>
public enum JobKind
{
    Compress,
    Extract,
}
