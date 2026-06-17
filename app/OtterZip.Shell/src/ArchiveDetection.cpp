// OtterZip.Shell — implementation of archive-extension detection.
// See ArchiveDetection.h for the rationale.

#include "ArchiveDetection.h"

#include <shlwapi.h>
#include <wrl/client.h>
#include <algorithm>

namespace OtterZip::Shell
{
    namespace
    {
        // Mirror of the archive ItemType list in Package.appxmanifest's
        // FileExplorerContextMenus block. If a new archive format lands
        // in the manifest, mirror it here too. (Lower-case .ext only;
        // HasArchiveExtension normalises before comparing.)
        constexpr wchar_t const* kArchiveExts[] = {
            L".zip",  L".zipx", L".7z",   L".rar",  L".tar",
            L".tgz",  L".tbz",  L".tbz2", L".tlz",  L".txz",  L".tzst",
            L".gz",   L".bz2",  L".xz",   L".lzma", L".zst",  L".lz4",
            L".jar",  L".war",  L".ear",  L".ipa",  L".apk",  L".aab",
            L".xpi",  L".crx",  L".iso",  L".img",  L".cab",  L".msi",
            L".deb",  L".appx", L".msix",
        };
    }

    bool HasArchiveExtension(std::wstring_view path) noexcept
    {
        auto dot = path.find_last_of(L'.');
        if (dot == std::wstring_view::npos) return false;
        std::wstring ext{ path.substr(dot) };
        std::transform(ext.begin(), ext.end(), ext.begin(),
            [](wchar_t c) { return static_cast<wchar_t>(::towlower(c)); });
        for (auto const* candidate : kArchiveExts)
        {
            if (ext == candidate) return true;
        }
        return false;
    }

    namespace
    {
        // Returns the absolute filesystem path of `item`, or empty for
        // virtual items. Shared helper used by both predicates.
        std::wstring ItemFileSystemPath(IShellItem* item) noexcept
        {
            if (!item) return {};
            LPWSTR raw = nullptr;
            if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, &raw)) || !raw)
            {
                return {};
            }
            std::wstring path{ raw };
            ::CoTaskMemFree(raw);
            return path;
        }
    }

    bool ItemsContainAnyArchive(IShellItemArray* items) noexcept
    {
        if (!items) return false;
        DWORD count = 0;
        if (FAILED(items->GetCount(&count)) || count == 0) return false;

        for (DWORD i = 0; i < count; ++i)
        {
            Microsoft::WRL::ComPtr<IShellItem> item;
            if (FAILED(items->GetItemAt(i, item.GetAddressOf()))) continue;
            std::wstring path = ItemFileSystemPath(item.Get());
            if (path.empty()) continue;
            if (HasArchiveExtension(path)) return true;
        }
        return false;
    }

    bool ItemsAreAllArchives(IShellItemArray* items) noexcept
    {
        if (!items) return false;
        DWORD count = 0;
        if (FAILED(items->GetCount(&count)) || count == 0) return false;

        // Need at least one item with a real path; virtual items
        // alone shouldn't trigger "all archives" lockout (they could
        // be folders shown via Library views, etc.).
        bool sawConcrete = false;
        for (DWORD i = 0; i < count; ++i)
        {
            Microsoft::WRL::ComPtr<IShellItem> item;
            if (FAILED(items->GetItemAt(i, item.GetAddressOf()))) return false;
            std::wstring path = ItemFileSystemPath(item.Get());
            if (path.empty()) continue;   // virtual / search-result item
            sawConcrete = true;
            if (!HasArchiveExtension(path)) return false;
        }
        return sawConcrete;
    }
}
