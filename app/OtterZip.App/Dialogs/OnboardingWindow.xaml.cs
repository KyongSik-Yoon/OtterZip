using System;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;
using System.Text.Json;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.Web.WebView2.Core;
using Microsoft.Windows.ApplicationModel.Resources;
using OtterZip.App.Services;
using OtterZip.Interop;
using Windows.Graphics;

namespace OtterZip.App.Dialogs;

/// <summary>
/// First-run onboarding — a borderless WebView2 window hosting the HTML
/// walkthrough at <c>Assets/Onboarding/index.html</c>. The HTML handles
/// step navigation; C# only (a) injects the localized strings + current
/// theme before first paint, and (b) handles the two side-effecting
/// actions the page posts back: opening Windows default-apps settings and
/// closing (which applies the crash-report opt-in). Completion is recorded
/// at show time by <see cref="OnboardingService.ShowIfFirstRun"/>.
/// </summary>
[System.Diagnostics.CodeAnalysis.SuppressMessage(
    "Design", "CA1001:Types that own disposable fields should be disposable",
    Justification = "WinUI 3 Window is not IDisposable; the WebView2 is torn down on Close.")]
public sealed partial class OnboardingWindow : Window
{
    // (JS key, .resw resource id) — the HTML reads window.OTTERZIP.strings.*
    // by the JS key, keeping .resw as the single translation source.
    private static readonly (string Js, string Res)[] StringKeys =
    {
        ("welcomeTitle", "Onboarding_WelcomeTitle/Text"),
        ("tagline", "Settings_Info_Tagline/Text"),
        ("featuresTitle", "Onboarding_FeaturesTitle/Text"),
        ("featRightClickTitle", "Onboarding_FeatRightClickTitle/Text"),
        ("featRightClickDesc", "Onboarding_FeatRightClickDesc/Text"),
        ("featDragTitle", "Onboarding_FeatDragTitle/Text"),
        ("featDragDesc", "Onboarding_FeatDragDesc/Text"),
        ("featDoubleClickTitle", "Onboarding_FeatDoubleClickTitle/Text"),
        ("featDoubleClickDesc", "Onboarding_FeatDoubleClickDesc/Text"),
        ("defaultTitle", "Onboarding_DefaultTitle/Text"),
        ("defaultDesc", "Onboarding_DefaultDesc/Text"),
        ("openDefaultApps", "Settings_OpenDefaultApps/Content"),
        ("crashTitle", "Onboarding_CrashTitle/Text"),
        ("crashDesc", "Onboarding_CrashDesc/Text"),
        ("crashToggle", "Onboarding_CrashToggle/Text"),
        ("doneTitle", "Onboarding_DoneTitle/Text"),
        ("doneDesc", "Onboarding_DoneDesc/Text"),
        ("skip", "Onboarding_SkipButton/Content"),
        ("back", "Onboarding_BackButton/Content"),
        ("start", "Onboarding_StartButton/Text"),
        ("next", "Onboarding_NextButton/Text"),
        ("finish", "Onboarding_FinishButton/Text"),
    };

    private readonly ResourceLoader _strings = new();

    public OnboardingWindow()
    {
        InitializeComponent();
        if (Content is FrameworkElement root)
        {
            ThemeService.Apply(root, ThemeService.Load());
        }
        Title = _strings.GetString("Onboarding_Title/Text");
        ConfigureWindow(580, 640);
        _ = InitializeWebViewAsync();
    }

    private async System.Threading.Tasks.Task InitializeWebViewAsync()
    {
        try
        {
            await WebView.EnsureCoreWebView2Async();
            CoreWebView2 core = WebView.CoreWebView2;

            // Lock the surface down to an app UI, not a browser.
            core.Settings.AreDefaultContextMenusEnabled = false;
            core.Settings.AreDevToolsEnabled = false;
            core.Settings.IsZoomControlEnabled = false;
            core.Settings.AreBrowserAcceleratorKeysEnabled = false;
            core.Settings.IsStatusBarEnabled = false;
            core.Settings.IsSwipeNavigationEnabled = false;

            string assetsDir = Path.Combine(AppContext.BaseDirectory, "Assets");
            core.SetVirtualHostNameToFolderMapping(
                "appassets.local", assetsDir, CoreWebView2HostResourceAccessKind.Allow);

            core.WebMessageReceived += OnWebMessage;
            await core.AddScriptToExecuteOnDocumentCreatedAsync(BuildInjectionScript());

            core.Navigate("https://appassets.local/Onboarding/index.html");
        }
        catch (Exception ex)
        {
            // WebView2 runtime missing / init failure — never trap the user
            // behind a blank borderless window they cannot close. Completion
            // is already marked at show time, so just close.
            Debug.WriteLine($"[OnboardingWindow] WebView init failed: {ex.Message}");
            Close();
        }
    }

    private string BuildInjectionScript()
    {
        string theme = (Content as FrameworkElement)?.ActualTheme == ElementTheme.Light ? "light" : "dark";
        bool optIn = SettingsService.Get<bool>("Settings_TelemetryEnabled", false);

        var sb = new StringBuilder(1024);
        sb.Append("window.OTTERZIP={theme:\"").Append(theme)
          .Append("\",crashOptIn:").Append(optIn ? "true" : "false")
          .Append(",strings:{");
        for (int i = 0; i < StringKeys.Length; i++)
        {
            if (i > 0)
            {
                sb.Append(',');
            }
            sb.Append('"').Append(StringKeys[i].Js).Append("\":\"")
              .Append(JsonEscape(_strings.GetString(StringKeys[i].Res))).Append('"');
        }
        sb.Append("}};");
        return sb.ToString();
    }

    private void OnWebMessage(CoreWebView2 sender, CoreWebView2WebMessageReceivedEventArgs args)
    {
        string payload;
        try
        {
            payload = args.TryGetWebMessageAsString();
        }
        catch (Exception)
        {
            return; // non-string message — ignore.
        }

        try
        {
            using var doc = JsonDocument.Parse(payload);
            string? action = doc.RootElement.TryGetProperty("action", out var a) ? a.GetString() : null;
            if (string.Equals(action, "openDefaultApps", StringComparison.Ordinal))
            {
                OpenDefaultApps();
            }
            else if (string.Equals(action, "close", StringComparison.Ordinal))
            {
                bool crash = doc.RootElement.TryGetProperty("crash", out var c)
                             && c.ValueKind == JsonValueKind.True;
                ApplyAndClose(crash);
            }
        }
        catch (JsonException)
        {
            // Malformed bridge message — ignore rather than crash onboarding.
        }
    }

    private void ApplyAndClose(bool crashOptIn)
    {
        try
        {
            SettingsService.Set("Settings_TelemetryEnabled", crashOptIn);
            OtterzipTelemetry.SetUserOptIn(crashOptIn);
        }
        catch (Exception)
        {
            // Telemetry wiring must never block the user from finishing.
        }
        Close();
    }

    private static void OpenDefaultApps()
    {
        try
        {
            Process.Start(new ProcessStartInfo
            {
                FileName = "ms-settings:defaultapps",
                UseShellExecute = true,
            });
        }
        catch (Exception)
        {
            // Server SKU / missing URI handler — non-fatal.
        }
    }

    /// <summary>Borderless (no title bar / min / max / close), fixed size,
    /// centered on the work area.</summary>
    private void ConfigureWindow(int width, int height)
    {
        try
        {
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = AppWindow.GetFromWindowId(windowId);

            if (appWindow.Presenter is OverlappedPresenter presenter)
            {
                presenter.IsResizable = false;
                presenter.IsMaximizable = false;
                presenter.IsMinimizable = false;
                presenter.SetBorderAndTitleBar(true, false);
            }

            uint dpi = GetDpiForWindow(hwnd);
            double scale = dpi / 96.0;
            int w = (int)(width * scale);
            int h = (int)(height * scale);

            var area = DisplayArea.GetFromWindowId(windowId, DisplayAreaFallback.Primary);
            int x = area.WorkArea.X + Math.Max(0, (area.WorkArea.Width - w) / 2);
            int y = area.WorkArea.Y + Math.Max(0, (area.WorkArea.Height - h) / 2);
            appWindow.MoveAndResize(new RectInt32(x, y, w, h));
        }
        catch (Exception)
        {
            // Best-effort chrome / sizing — fall back to OS defaults.
        }
    }

    private static string JsonEscape(string? value)
    {
        if (string.IsNullOrEmpty(value))
        {
            return string.Empty;
        }
        var sb = new StringBuilder(value.Length + 16);
        foreach (char ch in value)
        {
            switch (ch)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                // Escape markup-significant chars so a value can never break
                // out of the injected <script> string literal.
                case '<': sb.Append("\\u003c"); break;
                case '>': sb.Append("\\u003e"); break;
                case '&': sb.Append("\\u0026"); break;
                default:
                    if (ch < ' ')
                    {
                        sb.Append("\\u").Append(((int)ch).ToString("x4", CultureInfo.InvariantCulture));
                    }
                    else
                    {
                        sb.Append(ch);
                    }
                    break;
            }
        }
        return sb.ToString();
    }

    [System.Runtime.InteropServices.DllImport("user32.dll", ExactSpelling = true)]
    private static extern uint GetDpiForWindow(IntPtr hwnd);
}
