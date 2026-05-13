using System;
using System.Globalization;
using System.IO;
using System.Text;

namespace OtterZip.App.Services;

/// <summary>
/// Lightweight append-only log shared with the Rust tracing
/// subscriber. Same file path (<c>%TEMP%\otterzip\otterzip.log</c>) so
/// host-side and core-side phase markers interleave by wall-clock and
/// the user only has to share one file when reporting issues.
///
/// All writes go through a `lock` on a single static object — fine for
/// the ~tens of lines per extract this layer emits. Failures are
/// swallowed: diagnostic logging must never crash the host.
/// </summary>
internal static class DebugLog
{
    private static readonly System.Threading.Lock s_gate = new();
    private static readonly string s_path = ResolveLogPath();

    public static void Info(string message)
    {
        Write("INFO", message);
    }

    public static void Warn(string message)
    {
        Write("WARN", message);
    }

    private static void Write(string level, string message)
    {
        try
        {
            string now = DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.ffffffZ", CultureInfo.InvariantCulture);
            int tid = Environment.CurrentManagedThreadId;
            string line = $"{now}  {level} ThreadId({tid:00}) otterzip_app: {message}{Environment.NewLine}";
            lock (s_gate)
            {
                File.AppendAllText(s_path, line, Encoding.UTF8);
            }
        }
        catch
        {
            // Best-effort — never let logging crash the host.
        }
    }

    private static string ResolveLogPath()
    {
        try
        {
            string root = Environment.GetEnvironmentVariable("TEMP")
                ?? Environment.GetEnvironmentVariable("TMP")
                ?? Path.GetTempPath();
            string dir = Path.Combine(root, "otterzip");
            Directory.CreateDirectory(dir);
            return Path.Combine(dir, "otterzip.log");
        }
        catch
        {
            return Path.Combine(Path.GetTempPath(), "otterzip.log");
        }
    }
}
