namespace OtterZip.App.Models;

/// <summary>
/// Lifecycle states a <see cref="JobItem"/> moves through. State changes
/// raise PropertyChanged so the FloatLayer can react.
///
/// Linear happy path:
///   Queued → Running → Done
///
/// Failure / interrupt edges off Running:
///   Running → Cancelled (user pressed X)
///   Running → Error    (work threw)
///
/// Done / Cancelled / Error are terminal — the card lingers briefly for
/// follow-up actions then is reaped by the queue.
/// </summary>
public enum JobState
{
    Queued,
    Running,
    Done,
    Cancelled,
    Error,
}
