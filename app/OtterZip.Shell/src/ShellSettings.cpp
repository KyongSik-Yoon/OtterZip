// OtterZip.Shell — LocalSettings probe with short TTL cache.

#include "ShellSettings.h"

#include <atomic>
#include <chrono>
#include <mutex>
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

        CachedBool g_shellMenuEnabled;
        CachedBool g_extractHereDefault;

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
    }

    bool IsShellMenuEnabled() noexcept
    {
        return CachedRead(g_shellMenuEnabled, L"Settings_ShellMenuEnabled", /*default*/ true);
    }

    bool IsExtractHereDefault() noexcept
    {
        return CachedRead(g_extractHereDefault, L"Settings_ShellExtractHereAsDefault", /*default*/ true);
    }
}
