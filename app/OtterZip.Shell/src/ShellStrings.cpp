// OtterZip.Shell — ResourceLoader-backed string lookup.
//
// All shell IExplorerCommand methods (GetTitle / GetToolTip) route through
// `LoadShellString` instead of hardcoding L"..." literals.
//
// Why singleton-with-cache:
//   * `ResourceLoader::GetForViewIndependentUse()` itself is cheap, but
//     each `GetString` call goes through MRT (Modern Resource Technology)
//     which involves at least one COM hop. Verb hovers fire on every
//     right-click, so we cache successful lookups under a process-wide
//     mutex.
//   * Cache lookups are by UTF-16 key; misses cache a null sentinel so
//     a typo-key doesn't probe MRT on every hover.
//
// noexcept contract: a thrown winrt::hresult_error during ResourceLoader
// construction (rare — happens if package identity is not yet established
// in the surrogate process) is swallowed and the fallback is returned.
// Explorer never sees an exception escape from a verb method.

#include "ShellStrings.h"

#include <mutex>
#include <unordered_map>
#include <winrt/Windows.ApplicationModel.Resources.h>
#include <winrt/Windows.Foundation.h>

namespace OtterZip::Shell
{
    namespace
    {
        struct LookupCache
        {
            std::mutex mu;
            // `value.empty() && cached == true` represents a known-miss
            // — distinguished from "not yet attempted" by the cached
            // flag. Avoids re-probing MRT for missing keys on every
            // verb hover.
            struct Entry
            {
                std::wstring value;
                bool cached = false;
                bool hit = false;
            };
            std::unordered_map<std::wstring, Entry> entries;
        };

        LookupCache g_cache;

        // Lazily constructed; lifetime tied to the DLL. Re-creation on
        // first call after locale change is acceptable — Explorer
        // restarts the surrogate when the user switches locale anyway,
        // because the package identity (which carries language tags)
        // is re-resolved.
        std::mutex g_loaderMu;
        winrt::Windows::ApplicationModel::Resources::ResourceLoader g_loader{ nullptr };

        winrt::Windows::ApplicationModel::Resources::ResourceLoader EnsureLoader()
        {
            std::lock_guard<std::mutex> lock(g_loaderMu);
            if (!g_loader)
            {
                // `GetForViewIndependentUse()` returns the loader bound
                // to the package's default resource map. No view /
                // CoreApplication required, which matters because the
                // packaged-COM surrogate has neither.
                g_loader = winrt::Windows::ApplicationModel::Resources::ResourceLoader::GetForViewIndependentUse();
            }
            return g_loader;
        }
    }

    std::wstring LoadShellString(wchar_t const* key, std::wstring const& fallback) noexcept
    {
        if (key == nullptr || *key == L'\0')
        {
            return fallback;
        }
        std::wstring keyStr{ key };

        // Cache hit fast-path. We hold the mutex only for the lookup,
        // not for the (potentially blocking) MRT call.
        {
            std::lock_guard<std::mutex> lock(g_cache.mu);
            auto it = g_cache.entries.find(keyStr);
            if (it != g_cache.entries.end() && it->second.cached)
            {
                return it->second.hit ? it->second.value : fallback;
            }
        }

        std::wstring resolved;
        bool hit = false;
        try
        {
            auto loader = EnsureLoader();
            // `GetString` returns an empty hstring for a missing key
            // (not an exception). We treat empty as "miss" — every
            // shell-visible resw entry has non-empty content.
            winrt::hstring value = loader.GetString(winrt::hstring{ key });
            if (!value.empty())
            {
                resolved = std::wstring{ value.c_str() };
                hit = true;
            }
        }
        catch (...)
        {
            // ResourceLoader can throw during the brief window between
            // surrogate spin-up and PRI binding. Swallow and fall back.
            hit = false;
        }

        {
            std::lock_guard<std::mutex> lock(g_cache.mu);
            auto& entry = g_cache.entries[keyStr];
            entry.cached = true;
            entry.hit = hit;
            entry.value = resolved;
        }
        return hit ? resolved : fallback;
    }
}
