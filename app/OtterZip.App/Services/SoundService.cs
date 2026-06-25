using OtterZip.App.Models;

namespace OtterZip.App.Services;

/// <summary>
/// Plays a short completion sound when a job finishes successfully, gated
/// by the per-kind <c>Settings_PlaySoundOnCompress</c> /
/// <c>Settings_PlaySoundOnExtract</c> toggles.
///
/// <para>Uses Win32 <c>MessageBeep(MB_OK)</c> — the system "Default Beep"
/// event sound. Asset-free (no bundled .wav), AOT-safe, and it respects
/// the user's Windows sound scheme (silent if the user muted system
/// sounds). A bundled branded chime via <c>Windows.Media.Playback</c> can
/// replace this later without touching the call sites.</para>
/// </summary>
internal static class SoundService
{
    private const uint MbOk = 0x0000_0000;   // MB_OK → "Default Beep"

    [System.Runtime.InteropServices.DllImport("user32.dll", ExactSpelling = true)]
    private static extern bool MessageBeep(uint uType);

    /// <summary>
    /// Beep on successful completion if the matching per-kind setting is
    /// on. No-op for non-terminal/failed jobs. Never throws — sound is
    /// decoration and must not break the job flow. Call from any
    /// JobSettled handler unconditionally; the gating happens here.
    /// </summary>
    public static void MaybePlayCompletion(JobItem? item)
    {
        if (item is null || item.State != JobState.Done)
        {
            return;
        }
        string key = item.Kind == JobKind.Compress
            ? "Settings_PlaySoundOnCompress"
            : "Settings_PlaySoundOnExtract";
        if (!SettingsService.Get<bool>(key, true))
        {
            return;
        }
        try
        {
            MessageBeep(MbOk);
        }
        catch
        {
            // Best-effort — never let a sound failure surface to the user.
        }
    }
}
