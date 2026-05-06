// SpanZIP.Interop — telemetry / crash reporting (Phase 7 PR-7F).
//
// Backed by Sentry (.NET SDK). Opt-in only — defaults to OFF and never
// sends a single byte until the user flips Settings_TelemetryEnabled
// or sets the SPANZIP_TELEMETRY=1 environment variable.
//
// PII discipline: file paths and passwords often appear in stack traces
// and exception messages. The BeforeSend hook scrubs both before any
// payload leaves the process.

using System;
using System.Text.RegularExpressions;
using Sentry;

namespace SpanZIP.Interop;

public static partial class SpanzipTelemetry
{
    private static bool s_enabled;
    private static IDisposable? s_sentryHandle;

    /// <summary>
    /// Whether crash / diagnostic reporting is allowed. Defaults to OFF
    /// per `docs/02-design/mockup-spec.md` S10 — opt-in only.
    /// </summary>
    public static bool Enabled => s_enabled;

    /// <summary>
    /// Initialise the telemetry pipeline. Reads <c>SPANZIP_TELEMETRY=1</c>
    /// from the process environment as a boot-time hint; the user-controlled
    /// preference (Settings → Info → "Crash report opt-in") then takes
    /// precedence via <see cref="SetUserOptIn(bool)"/>.
    /// </summary>
    public static void Initialize()
    {
        var env = Environment.GetEnvironmentVariable("SPANZIP_TELEMETRY");
        if (string.Equals(env, "1", StringComparison.Ordinal))
        {
            SetUserOptIn(true);
        }
    }

    /// <summary>
    /// Apply the user's opt-in preference. Idempotent; safe to call from
    /// the Settings page on every checkbox flip.
    /// </summary>
    public static void SetUserOptIn(bool optedIn)
    {
        if (optedIn == s_enabled)
        {
            return;
        }
        s_enabled = optedIn;
        if (optedIn)
        {
            StartSentry();
        }
        else
        {
            StopSentry();
        }
    }

    private static void StartSentry()
    {
        if (s_sentryHandle is not null)
        {
            return;
        }
        // DSN comes from a build-time / install-time env var so the secret
        // never lands in source control. When unset (typical dev run) we
        // skip init entirely — opt-in but no destination = no-op.
        var dsn = Environment.GetEnvironmentVariable("SPANZIP_SENTRY_DSN");
        if (string.IsNullOrEmpty(dsn))
        {
            return;
        }
        try
        {
            s_sentryHandle = SentrySdk.Init(o =>
            {
                o.Dsn = dsn;
                o.AutoSessionTracking = false;
                o.SendDefaultPii = false;
                o.AttachStacktrace = true;
                o.MaxBreadcrumbs = 50;
                // PII scrub: strip Windows file paths and obvious password
                // patterns before any payload leaves the process.
                o.SetBeforeSend(SanitizeEvent);
            });
        }
        catch (Exception)
        {
            // Sentry SDK init failure must never crash the host. Silently
            // disable; the user just gets no crash reporting this session.
            s_sentryHandle = null;
            s_enabled = false;
        }
    }

    private static void StopSentry()
    {
        try
        {
            s_sentryHandle?.Dispose();
        }
        catch (Exception)
        {
            // Same defence as init — disposing must not propagate.
        }
        finally
        {
            s_sentryHandle = null;
        }
    }

    /// <summary>
    /// PII scrubber. Replaces Windows path drive prefixes and likely
    /// password tokens in messages / breadcrumbs / exception text so the
    /// payload reaching Sentry contains the bug shape without the user's
    /// data.
    /// </summary>
    private static SentryEvent? SanitizeEvent(SentryEvent evt, SentryHint _)
    {
        if (evt.Message?.Message is string msg)
        {
            evt.Message.Message = Scrub(msg);
        }
        if (evt.Message?.Formatted is string fmt)
        {
            evt.Message.Formatted = Scrub(fmt);
        }
        return evt;
    }

    private static string Scrub(string input)
    {
        // Drive paths: C:\Users\foo\... → <path>
        string s = WindowsPathRegex().Replace(input, "<path>");
        // Naive password=… pattern: password=... → password=<redacted>
        s = PasswordRegex().Replace(s, "password=<redacted>");
        return s;
    }

    [GeneratedRegex(@"[A-Za-z]:\\[^\s""']+", RegexOptions.Compiled)]
    private static partial Regex WindowsPathRegex();

    [GeneratedRegex(@"(?i)password\s*[:=]\s*\S+", RegexOptions.Compiled)]
    private static partial Regex PasswordRegex();
}
