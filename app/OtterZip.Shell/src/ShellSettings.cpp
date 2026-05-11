// OtterZip.Shell — LocalSettings probe with short TTL cache.

#include "ShellSettings.h"

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
        constexpr std::chrono::seconds kCacheTtl{ 5 };

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
        CachedBool g_extractHereDefault;
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
        return CachedRead(g_shellMenuEnabled, L"Settings_ShellMenuEnabled", /*default*/ true);
    }

    bool IsExtractHereDefault() noexcept
    {
        return CachedRead(g_extractHereDefault, L"Settings_ShellExtractHereAsDefault", /*default*/ true);
    }

    bool IsShellMenuNested() noexcept
    {
        // Default mode = "nested" — matches the C# Settings default
        // (IntegrationSettingsSection.xaml.cs reads with same default)
        // and aligns with the Windows 11 / 7-Zip / WinRAR convention of
        // grouping app verbs under a single labelled parent. Users who
        // prefer the legacy "flat" layout can flip the radio in
        // Settings -> Integration tab.
        auto mode = CachedReadString(g_shellMenuMode, L"Settings_ShellMenuMode", L"nested");
        return mode == L"nested";
    }
}
