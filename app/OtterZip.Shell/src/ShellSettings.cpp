// OtterZip.Shell — LocalSettings probe with short TTL cache.
//
// 2026-06-18: a pure-Win32 LocalState cache fast-path (ShellCache) is tried
// first. It avoids the AppX activation context that the LocalSettings RPC
// below would establish — the cold-boot first-right-click miss. The WinRT
// path here remains as the fallback when the cache file is absent.

#include "ShellSettings.h"
#include "ShellCache.h"

#include <atomic>
#include <chrono>
#include <mutex>
#include <string>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Storage.h>

namespace OtterZip::Shell
{
    namespace
    {
        // TTL extended from 5s to 60s on 2026-05-19 to address verb
        // cold-start invisibility: every right-click was re-fetching
        // LocalSettings (cross-process RPC, ~50ms) for each of 9 verbs.
        // 60s keeps Settings UI toggles responsive (user flips the
        // radio → next right-click after a minute reflects it) while
        // making the steady-state hover essentially free.
        constexpr std::chrono::seconds kCacheTtl{ 60 };

        struct CachedBool
        {
            std::mutex mu;
            std::chrono::steady_clock::time_point fetchedAt{};
            bool value = true;
            bool initialized = false;
        };

        // ShellMenuMode is persisted as a string ("flat" / "nested"),
        // so it gets its own cache slot with a string-aware reader.
        struct CachedString
        {
            std::mutex mu;
            std::chrono::steady_clock::time_point fetchedAt{};
            std::wstring value;
            bool initialized = false;
        };

        CachedBool g_shellMenuEnabled;
        CachedString g_shellMenuMode;

        // Reads a single bool from ApplicationData::LocalSettings.
        // Returns `defaultValue` when:
        //   * the key isn't present yet (first run),
        //   * the package identity isn't established (host process is
        //     unpackaged dev build invoking us via regsvr32),
        //   * any WinRT activation throws.
        bool ReadBoolFromLocalSettings(wchar_t const* key, bool defaultValue) noexcept
        {
            try
            {
                auto settings = winrt::Windows::Storage::ApplicationData::Current().LocalSettings();
                auto values = settings.Values();
                if (!values.HasKey(key))
                {
                    return defaultValue;
                }
                auto raw = values.Lookup(key);
                if (auto box = raw.try_as<winrt::Windows::Foundation::IReference<bool>>())
                {
                    return box.Value();
                }
                return defaultValue;
            }
            catch (...)
            {
                return defaultValue;
            }
        }

        bool CachedRead(CachedBool& slot, wchar_t const* key, bool defaultValue) noexcept
        {
            std::lock_guard<std::mutex> lock(slot.mu);
            auto now = std::chrono::steady_clock::now();
            if (slot.initialized && (now - slot.fetchedAt) < kCacheTtl)
            {
                return slot.value;
            }
            slot.value = ReadBoolFromLocalSettings(key, defaultValue);
            slot.fetchedAt = now;
            slot.initialized = true;
            return slot.value;
        }

        // String reader, mirroring ReadBoolFromLocalSettings shape.
        std::wstring ReadStringFromLocalSettings(wchar_t const* key, std::wstring const& defaultValue) noexcept
        {
            try
            {
                auto settings = winrt::Windows::Storage::ApplicationData::Current().LocalSettings();
                auto values = settings.Values();
                if (!values.HasKey(key))
                {
                    return defaultValue;
                }
                auto raw = values.Lookup(key);
                if (auto box = raw.try_as<winrt::Windows::Foundation::IReference<winrt::hstring>>())
                {
                    return std::wstring{ box.Value().c_str() };
                }
                return defaultValue;
            }
            catch (...)
            {
                return defaultValue;
            }
        }

        std::wstring CachedReadString(CachedString& slot, wchar_t const* key, std::wstring const& defaultValue) noexcept
        {
            std::lock_guard<std::mutex> lock(slot.mu);
            auto now = std::chrono::steady_clock::now();
            if (slot.initialized && (now - slot.fetchedAt) < kCacheTtl)
            {
                return slot.value;
            }
            slot.value = ReadStringFromLocalSettings(key, defaultValue);
            slot.fetchedAt = now;
            slot.initialized = true;
            return slot.value;
        }
    }

    bool IsShellMenuEnabled() noexcept
    {
        bool cached;
        if (TryGetCachedBool(L"ShellMenuEnabled", cached))
        {
            return cached;
        }
        return CachedRead(g_shellMenuEnabled, L"Settings_ShellMenuEnabled", /*default*/ true);
    }

    bool IsShellMenuNested() noexcept
    {
        // Default mode = "flat" (Bandizip parity, re-attempted 2026-05-19
        // on a clean v0.20.2 build after the v0.13 flat-default attempt
        // was rolled back due to packaged-COM verb registration silently
        // disappearing — that turned out to be a stale Shell DLL bundling
        // bug in build-msix.bat / vcxproj OutDir, fixed in the same
        // commit as this default flip; see OtterZip.Shell.vcxproj's
        // OutDir comment).
        //
        // Behaviour:
        //   * Fresh install with no LocalSettings_ShellMenuMode → "flat"
        //     fallback → quick verbs (`<basename>.zip으로 압축`,
        //     `<basename>.7z으로 압축`, `OtterZip으로 압축...`, Extract
        //     here) render at the right-click root next to Bandizip.
        //   * `OtterzipMenuCommand::GetState` checks the same key and
        //     hides its "OtterZip ▶" parent in flat mode, so the two
        //     layouts never double-render.
        //   * Users who prefer the v0.13-and-earlier "OtterZip ▶"
        //     submenu can flip the radio in Settings → Integration tab.
        //     `IntegrationSettingsSection.xaml.cs` writes the persisted
        //     string, the cache TTL (5 s) refreshes from LocalSettings,
        //     and `IsShellMenuNested` flips on the next verb hover.
        std::wstring cachedMode;
        if (TryGetCachedString(L"ShellMenuMode", cachedMode))
        {
            return cachedMode == L"nested";
        }
        auto mode = CachedReadString(g_shellMenuMode, L"Settings_ShellMenuMode", L"flat");
        return mode == L"nested";
    }
}
