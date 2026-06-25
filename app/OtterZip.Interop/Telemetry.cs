// OtterZip.Interop — telemetry / crash reporting (Phase 7 PR-7F).
//
// Backed by Sentry (.NET SDK). Opt-in only — defaults to OFF and never
// sends a single byte until the user flips Settings_TelemetryEnabled
// or sets the OTTERZIP_TELEMETRY=1 environment variable.
//
// PII discipline: file paths and passwords often appear in stack traces
// and exception messages. The BeforeSend hook scrubs both before any
// payload leaves the process.

using System;
using System.Text.RegularExpressions;
using Sentry;

namespace OtterZip.Interop;

public static partial class OtterzipTelemetry
{
    private static bool s_enabled;
    private static IDisposable? s_sentryHandle;
    private static string? s_installId;

    // Shipped Sentry DSN (LumiBear Studio → otterzip project, EU region).
    // A DSN is a public, write-only ingestion key — not a secret — so
    // embedding it in the client is the standard Sentry pattern. Override
    // at runtime with the OTTERZIP_SENTRY_DSN env var (dev / staging /
    // self-host) when a different destination is wanted.
    private const string DefaultDsn =
        "https://35ca273af6a07fd1e6811f263ba51b3a@o4510949994266624.ingest.de.sentry.io/4511606044950608";

    /// <summary>
    /// Whether crash / diagnostic reporting is allowed. Defaults to OFF
    /// per `docs/02-design/mockup-spec.md` S10 — opt-in only.
    /// </summary>
    public static bool Enabled => s_enabled;

    /// <summary>
    /// Initialise the telemetry pipeline. Reads <c>OTTERZIP_TELEMETRY=1</c>
    /// from the process environment as a boot-time hint; the user-controlled
    /// preference (Settings → Info → "Crash report opt-in") then takes
    /// precedence via <see cref="SetUserOptIn(bool)"/>.
    /// </summary>
    public static void Initialize()
    {
        var env = Environment.GetEnvironmentVariable("OTTERZIP_TELEMETRY");
        if (string.Equals(env, "1", StringComparison.Ordinal))
        {
            SetUserOptIn(true);
        }
    }

    /// <summary>
    /// The anonymous per-install id (random GUID, not tied to any identity),
    /// surfaced in Settings → Info → Build so a user can quote it when asking
    /// us to delete their crash reports. Empty until <see cref="SetInstallId"/>
    /// is called at startup.
    /// </summary>
    public static string InstallId => s_installId ?? string.Empty;

    /// <summary>
    /// Record the anonymous install id. Tags every Sentry event with it (so
    /// reports can be located for a deletion request) and applies it
    /// retroactively if Sentry is already running. Call once at startup,
    /// before <see cref="Initialize"/>.
    /// </summary>
    public static void SetInstallId(string id)
    {
        s_installId = id;
        if (s_sentryHandle is not null && !string.IsNullOrEmpty(id))
        {
            try
            {
                SentrySdk.ConfigureScope(scope => scope.SetTag("install_id", id));
            }
            catch (Exception)
            {
                // Telemetry must never throw into the caller.
            }
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

    /// <summary>
    /// Forward an exception to Sentry. No-op when the user hasn't opted in
    /// or Sentry never initialised (e.g. empty DSN). Safe to call from a
    /// crash handler — it never throws and never blocks.
    /// </summary>
    public static void CaptureException(Exception? ex)
    {
        if (!s_enabled || s_sentryHandle is null || ex is null)
        {
            return;
        }
        try
        {
            SentrySdk.CaptureException(ex);
        }
        catch (Exception)
        {
            // A crash reporter must never crash the host it reports for.
        }
    }

    /// <summary>
    /// Report a failed compress/extract operation, classified so the
    /// dashboard separates OUR bugs from expected user errors:
    /// <list type="bullet">
    /// <item>Rust panic (-1) / invalid arg (-2) / invalid handle (-3) →
    ///   <c>category=bug</c>, level Error.</item>
    /// <item>Backend error (-50) → <c>category=backend</c>, level Warning
    ///   (ambiguous: a library defect or genuinely bad data).</item>
    /// <item>Everything else (-10..-42: wrong password, corrupt, disk full,
    ///   unsupported, path-traversal, zip-bomb) → <c>category=user</c>,
    ///   level Info (frequency signal, not a bug).</item>
    /// <item>Cancellation → never reported.</item>
    /// </list>
    /// Only safe metadata is attached (operation, error code, archive
    /// format hint); paths / names / passwords are scrubbed by BeforeSend.
    /// No-op until the user opts in. Never throws.
    /// </summary>
    public static void ReportOperationFailure(string operation, Exception? ex, string? archiveFormat = null)
    {
        if (!s_enabled || s_sentryHandle is null || ex is null)
        {
            return;
        }
        if (ex is OperationCanceledException)
        {
            return;
        }

        int code = (ex as OtterzipException)?.ErrorCode ?? 0;
        if (!ClassifyFailure(ex, code, out string category, out SentryLevel level))
        {
            return; // user cancel — never reported
        }

        try
        {
            var inv = System.Globalization.CultureInfo.InvariantCulture;
            var evt = new SentryEvent(ex) { Level = level };
            evt.SetTag("operation", operation);
            evt.SetTag("error_category", category);
            evt.SetTag("error_code", code.ToString(inv));
            if (!string.IsNullOrEmpty(archiveFormat))
            {
                evt.SetTag("archive_format", archiveFormat);
            }
            evt.SetExtra("exception_type", ex.GetType().Name);
            // Group by (operation, category, code, format) so each distinct
            // failure mode is one issue, not thousands keyed on the message.
            evt.SetFingerprint(new[] { operation, category, code.ToString(inv), archiveFormat ?? "any" });
            SentrySdk.CaptureEvent(evt);
        }
        catch (Exception)
        {
            // A reporter must never crash the operation it reports on.
        }
    }

    /// <summary>
    /// Bucket a failed-operation exception into (category, Sentry level).
    /// Returns false when the failure must NOT be reported (user cancel).
    /// </summary>
    private static bool ClassifyFailure(Exception ex, int code, out string category, out SentryLevel level)
    {
        category = "app";
        level = SentryLevel.Error;
        if (ex is not OtterzipException)
        {
            // Unexpected C#-side defect / unhandled IO edge → treat as a bug.
            return true;
        }
        switch (code)
        {
            case -30:                 // user cancel — never report
                return false;
            case -1:                  // Rust panic surfaced through the FFI
            case -2:                  // invalid argument from our wrapper
            case -3:                  // invalid/stale native handle
                category = "bug";
                return true;
            case -50:                 // backend lib error — ambiguous
                category = "backend";
                level = SentryLevel.Warning;
                return true;
            default:                  // -10..-42 expected user/archive errors
                category = "user";
                level = SentryLevel.Info;
                return true;
        }
    }

    private static void StartSentry()
    {
        if (s_sentryHandle is not null)
        {
            return;
        }
        // OTTERZIP_SENTRY_DSN overrides for dev / staging / self-host;
        // otherwise the shipped DSN above is used. Only a genuinely empty
        // DSN (both unset) skips init entirely — opt-in with no destination.
        var dsn = Environment.GetEnvironmentVariable("OTTERZIP_SENTRY_DSN");
        if (string.IsNullOrEmpty(dsn))
        {
            dsn = DefaultDsn;
        }
        if (string.IsNullOrEmpty(dsn))
        {
            return;
        }
        try
        {
            s_sentryHandle = SentrySdk.Init(o => ConfigureSentryOptions(o, dsn));
        }
        catch (Exception)
        {
            // Sentry SDK init failure must never crash the host. Silently
            // disable; the user just gets no crash reporting this session.
            s_sentryHandle = null;
            s_enabled = false;
        }
    }

    /// <summary>
    /// Configure the Sentry SDK for privacy-first, opt-in crash reporting.
    /// Extracted from <see cref="StartSentry"/> so each stays under the
    /// MA0051 method-length cap.
    /// </summary>
    private static void ConfigureSentryOptions(SentryOptions o, string dsn)
    {
        o.Dsn = dsn;
        // OTTERZIP_TELEMETRY_DEBUG=1 turns on Sentry's local debug log so a user
        // can inspect exactly what would be sent before it leaves the device
        // (documented in PRIVACY.md → "Your choices").
        o.Debug = string.Equals(
            Environment.GetEnvironmentVariable("OTTERZIP_TELEMETRY_DEBUG"),
            "1", StringComparison.Ordinal);
        o.AutoSessionTracking = false;
        // Never send IP / username / machine identity. (The Sentry project
        // should ALSO enable "Prevent Storing of IP Addresses" server-side —
        // see docs/08-deployment/sentry-privacy.md.)
        o.SendDefaultPii = false;
        // Report a constant instead of the SDK default (Environment.MachineName)
        // so the user's hostname never leaks.
        o.ServerName = "OtterZip";
        // Accurate, disclosed version reporting + a fixed environment label.
        o.Release = "otterzip@" + AppVersion();
        o.Environment = "production";
        // Anonymous per-install id so reports can be located for a deletion
        // request. Not tied to any identity.
        if (!string.IsNullOrEmpty(s_installId))
        {
            o.DefaultTags["install_id"] = s_installId!;
        }
        o.AttachStacktrace = true;
        // Breadcrumbs are NOT reachable from SetBeforeSend's scrub (the event's
        // breadcrumb collection is read-only in the hook), so an auto-captured
        // log / HTTP breadcrumb would bypass PII scrubbing. Disable them
        // entirely — OtterZip writes none of its own.
        o.MaxBreadcrumbs = 0;
        // PII scrub: strip paths + password tokens before any payload leaves.
        o.SetBeforeSend(SanitizeEvent);
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

    /// <summary>MAJOR.MINOR.PATCH of this assembly, for the Sentry release.</summary>
    private static string AppVersion()
    {
        var v = typeof(OtterzipTelemetry).Assembly.GetName().Version;
        if (v is null)
        {
            return "0.0.0";
        }
        var inv = System.Globalization.CultureInfo.InvariantCulture;
        return v.Major.ToString(inv) + "." + v.Minor.ToString(inv) + "." + v.Build.ToString(inv);
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
        // Exception values carry file paths (e.g. "File not found: C:\...")
        // far more often than the top-level message, so scrub each one too.
        if (evt.SentryExceptions is not null)
        {
            foreach (var sentryEx in evt.SentryExceptions)
            {
                if (sentryEx.Value is not string exVal)
                {
                    continue;
                }
                // OtterzipException messages are built verbatim from the native
                // OtterzipError Display strings, which embed archive ENTRY names
                // and relative / UNC paths (e.g. "entry not found: Clients/Acme/
                // contract.pdf", "path traversal rejected: ../secret.xlsx") that
                // the drive-path regex below cannot match. Redact those wholesale
                // — the error_code / error_category tags keep the actionable
                // signal without leaking the user's filenames.
                sentryEx.Value =
                    sentryEx.Type is string t
                    && t.Contains("OtterzipException", StringComparison.Ordinal)
                        ? "<redacted>"
                        : Scrub(exVal);
            }
        }
        return evt;
    }

    private static string Scrub(string input)
    {
        // Drive paths: C:\Users\foo\... → <path>
        string s = WindowsPathRegex().Replace(input, "<path>");
        // UNC paths: \\server\share\... → <path>
        s = UncPathRegex().Replace(s, "<path>");
        // Naive password=… pattern: password=... → password=<redacted>
        s = PasswordRegex().Replace(s, "password=<redacted>");
        return s;
    }

    [GeneratedRegex(@"[A-Za-z]:\\[^\s""']+", RegexOptions.Compiled)]
    private static partial Regex WindowsPathRegex();

    [GeneratedRegex(@"\\\\[^\s""']+", RegexOptions.Compiled)]
    private static partial Regex UncPathRegex();

    [GeneratedRegex(@"(?i)password\s*[:=]\s*\S+", RegexOptions.Compiled)]
    private static partial Regex PasswordRegex();
}
