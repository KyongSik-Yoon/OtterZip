// OtterZip.Shell — Bandizip-style direct format-specific compress verbs.
//
// See QuickCompressCommands.h for the architecture rationale.

#include "QuickCompressCommands.h"
#include "ArchiveDetection.h"
#include "ShellAssets.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"
#include "ShellStrings.h"

#include <shellapi.h>
#include <shlwapi.h>
#include <pathcch.h>
#include <wrl/client.h>
#include <string>

#pragma comment(lib, "Shlwapi.lib")
#pragma comment(lib, "Pathcch.lib")

namespace OtterZip::Shell
{
    // -----------------------------------------------------------------
    // DeriveSelectionStem — pick the basename for the verb title.
    //
    // Mirrors `OutputNamer.SourceStem` (host side) on a single item.
    // For multi-select we use the first selected item's basename —
    // that's a "preview hint", the host's OutputNamer will compute
    // the final stem (which may differ if Settings_UseParentFolderName
    // is on). The mismatch only manifests on multi-select, and in that
    // case Bandizip itself also shows one of the selected names — not
    // the eventual parent-folder name. Acceptable trade-off for not
    // synchronously reading the host's settings store from a verb
    // hover.
    // -----------------------------------------------------------------
    namespace
    {
        // Returns the basename of `path`'s parent directory.
        //   "D:\\Photos\\img.jpg" -> "Photos"
        //   "D:\\img.jpg"          -> "" (root drive — caller fallback)
        //   "\\\\server\\share\\f" -> "share"
        // Pure string manipulation; no filesystem I/O.
        std::wstring ParentDirName(std::wstring const& path) noexcept
        {
            if (path.empty()) return {};
            // Trim trailing separator (rare, but defensive).
            size_t end = path.size();
            while (end > 0 && (path[end - 1] == L'\\' || path[end - 1] == L'/'))
            {
                --end;
            }
            if (end == 0) return {};
            // Find the separator before the last component.
            size_t lastSep = path.find_last_of(L"\\/", end - 1);
            if (lastSep == std::wstring::npos) return {};
            // The parent directory's name lives between the previous
            // separator (if any) and `lastSep`.
            size_t prevSep = (lastSep == 0) ? std::wstring::npos
                                            : path.find_last_of(L"\\/", lastSep - 1);
            size_t nameStart = (prevSep == std::wstring::npos) ? 0 : prevSep + 1;
            // Skip drive-letter (e.g. "D:") if it would be the whole parent.
            if (nameStart + 1 < lastSep && path[nameStart + 1] == L':')
            {
                // The parent IS the drive letter — return empty so caller
                // falls back to the first item's stem.
                return {};
            }
            return path.substr(nameStart, lastSep - nameStart);
        }

        // Display-name retrieval shared by single + multi paths.
        // Tries SIGDN_PARENTRELATIVEFORADDRESSBAR first, falls back to
        // SIGDN_NORMALDISPLAY. Returns empty on hard failure.
        std::wstring DisplayName(IShellItem* item) noexcept
        {
            if (item == nullptr) return {};
            LPWSTR raw = nullptr;
            HRESULT hr = item->GetDisplayName(SIGDN_PARENTRELATIVEFORADDRESSBAR, &raw);
            if (FAILED(hr) || raw == nullptr)
            {
                hr = item->GetDisplayName(SIGDN_NORMALDISPLAY, &raw);
            }
            if (FAILED(hr) || raw == nullptr) return {};
            std::wstring name(raw);
            ::CoTaskMemFree(raw);
            return name;
        }

        // Full-path retrieval — `SIGDN_FILESYSPATH` returns the absolute
        // filesystem path or empty for virtual items. Required to derive
        // the parent-directory name for multi-select labels.
        std::wstring FileSystemPath(IShellItem* item) noexcept
        {
            if (item == nullptr) return {};
            LPWSTR raw = nullptr;
            HRESULT hr = item->GetDisplayName(SIGDN_FILESYSPATH, &raw);
            if (FAILED(hr) || raw == nullptr) return {};
            std::wstring path(raw);
            ::CoTaskMemFree(raw);
            return path;
        }

        // Strip the file extension so `foo.exe` → `foo`. Directory
        // selections have no extension to strip — return unchanged.
        std::wstring StripExtension(std::wstring name) noexcept
        {
            wchar_t const* ext = ::PathFindExtensionW(name.c_str());
            if (ext != nullptr && *ext == L'.')
            {
                name.resize(static_cast<size_t>(ext - name.c_str()));
            }
            return name;
        }
    }

    std::wstring DeriveSelectionStem(IShellItemArray* items) noexcept
    {
        if (items == nullptr) return L"Archive";
        DWORD count = 0;
        if (FAILED(items->GetCount(&count)) || count == 0) return L"Archive";

        Microsoft::WRL::ComPtr<IShellItem> first;
        if (FAILED(items->GetItemAt(0, first.GetAddressOf())) || first == nullptr)
        {
            return L"Archive";
        }

        // ---- Multi-select path: use parent-directory name. -----------
        //
        // Bandizip parity (verified 2026-05-19 by capturing the actual
        // right-click menu on a sibling selection): when the user
        // multi-selects N files Bandizip labels the quick verb with
        // the parent folder's name. That's the natural archive name
        // for "pack the files I'm looking at" — using just the first
        // item's stem (`readme` for `readme.txt + script.js`) is
        // misleading.
        //
        // Failure modes covered:
        //   * Mixed parents (rare — happens via Explorer Search or
        //     Library views): fall through to first-item stem.
        //   * Parent IS a root drive (`D:\\`): ParentDirName returns
        //     empty → fall through. Avoids the absurd `D.zip`.
        //   * Virtual items with no filesystem path: same fallback.
        if (count > 1)
        {
            std::wstring firstPath = FileSystemPath(first.Get());
            if (!firstPath.empty())
            {
                std::wstring parent = ParentDirName(firstPath);
                if (!parent.empty())
                {
                    // Verify all other items share the same parent. If
                    // a single item differs we fall back rather than
                    // mislabel — better to ship a `readme.zip` for an
                    // ambiguous selection than `archive.zip` that
                    // hides what's being packed.
                    bool allSameParent = true;
                    for (DWORD i = 1; i < count; ++i)
                    {
                        Microsoft::WRL::ComPtr<IShellItem> next;
                        if (FAILED(items->GetItemAt(i, next.GetAddressOf())) || next == nullptr)
                        {
                            allSameParent = false;
                            break;
                        }
                        std::wstring nextPath = FileSystemPath(next.Get());
                        if (nextPath.empty() || ParentDirName(nextPath) != parent)
                        {
                            allSameParent = false;
                            break;
                        }
                    }
                    if (allSameParent)
                    {
                        return parent;
                    }
                }
            }
            // Fall through to first-item stem for mixed / root / virtual.
        }

        // ---- Single-select path: first item's stem (original logic). -
        std::wstring name = DisplayName(first.Get());
        if (name.empty()) return L"Archive";
        std::wstring stem = StripExtension(std::move(name));
        return stem.empty() ? std::wstring(L"Archive") : stem;
    }

    // -----------------------------------------------------------------
    // Generic shape — both classes have identical GetState / GetFlags /
    // GetIcon / GetToolTip / EnumSubCommands; only GetTitle and Invoke
    // differ between ZIP and 7z. Helpers below capture the shared bits.
    // -----------------------------------------------------------------

    namespace
    {
        HRESULT GetSharedState(IShellItemArray* items, EXPCMDSTATE* pCmdState) noexcept
        {
            if (pCmdState == nullptr) return E_POINTER;
            if (!IsShellMenuEnabled())
            {
                *pCmdState = ECS_HIDDEN;
                return S_OK;
            }
            // Nested-mode mirror of CompressCommand:51-54. Without
            // this gate, nested-mode users would see the OtterZip
            // parent menu AND three loose top-level quick verbs at
            // once.
            if (IsShellMenuNested())
            {
                *pCmdState = ECS_HIDDEN;
                return S_OK;
            }
            // Bandizip parity (spec §E): hide Compress verbs ONLY when
            // every item in the selection is an archive. Mixed
            // archive + non-archive selections still show Compress
            // — the user typically wants to pack the whole bag, not
            // extract part of it. Earlier `ItemsContainAnyArchive`
            // was too aggressive and hid all OtterZip verbs the
            // moment a single .zip joined the selection.
            if (ItemsAreAllArchives(items))
            {
                *pCmdState = ECS_HIDDEN;
                return S_OK;
            }
            DWORD count = 0;
            if (items && SUCCEEDED(items->GetCount(&count)) && count > 0)
            {
                *pCmdState = ECS_ENABLED;
            }
            else
            {
                *pCmdState = ECS_HIDDEN;
            }
            return S_OK;
        }

        // Compose `<basename>.<ext>으로 압축(&K)` via locale-aware
        // format. The .resw key `Shell_CompressQuick_TitleFormat`
        // holds the format string (`{0}{1}으로 압축(&{2})` for Korean,
        // `Compress to {0}{1} (&{2})` for English, etc.). We expand
        // it ourselves because `printf("%1!s!", arg)`-style insertion
        // markers add a layer of complexity that's not worth it for
        // three placeholders — straightforward `{0}` / `{1}` / `{2}`
        // substitution in C++ covers every locale we ship.
        //
        // Hotkey letter is locale-shared (ZIP→Z, 7z→7) so it does not
        // need translation — Bandizip behaves the same. Extension
        // (`.zip` / `.7z`) is also language-neutral.
        HRESULT FormatTitle(
            IShellItemArray* items,
            wchar_t const* extWithDot,
            wchar_t hotkey,
            LPWSTR* ppszName) noexcept
        {
            std::wstring stem = DeriveSelectionStem(items);
            // Default (Korean) preserves v0.15-v0.19 behavior exactly
            // for the case where MRT lookup fails. Other locales come
            // through the .resw files defined in Strings/<locale>/.
            std::wstring fmt = LoadShellString(
                L"Shell_CompressQuick_TitleFormat",
                L"{0}{1}으로 압축(&{2})");
            // Substitute placeholders. `{0}` = stem, `{1}` = extension
            // (.zip / .7z), `{2}` = hotkey letter. Plain string replace
            // because we don't want a heavyweight std::format dance
            // for three deterministic substitutions on a hot verb
            // hover path.
            auto replace = [](std::wstring& s, std::wstring const& token, std::wstring const& v)
            {
                size_t pos = s.find(token);
                if (pos != std::wstring::npos)
                {
                    s.replace(pos, token.size(), v);
                }
            };
            replace(fmt, L"{0}", stem);
            replace(fmt, L"{1}", std::wstring{ extWithDot });
            replace(fmt, L"{2}", std::wstring(1, hotkey));
            return SHStrDupW(fmt.c_str(), ppszName);
        }
    }

    // ================================================================
    // CompressZipQuickCommand
    // ================================================================

    IFACEMETHODIMP CompressZipQuickCommand::GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept
    {
        return FormatTitle(items, L".zip", L'Z', ppszName);
    }
    IFACEMETHODIMP CompressZipQuickCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Shared brand icon path — same rationale as the other top-level
        // verbs. Per-format glyphs (zip vs 7z) intentionally NOT used here:
        // user picked the "single OtterZip brand mark on every entry"
        // scheme for v0.20.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }
    IFACEMETHODIMP CompressZipQuickCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_CompressZipQuick_Tooltip",
            L"Compress selected items to a ZIP archive (no dialog).");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP CompressZipQuickCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(CompressZipQuickCommand);
        return S_OK;
    }
    IFACEMETHODIMP CompressZipQuickCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        return GetSharedState(items, pCmdState);
    }
    IFACEMETHODIMP CompressZipQuickCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"compress-zip");
    }
    IFACEMETHODIMP CompressZipQuickCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP CompressZipQuickCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // ================================================================
    // CompressSevenZQuickCommand
    // ================================================================

    IFACEMETHODIMP CompressSevenZQuickCommand::GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept
    {
        return FormatTitle(items, L".7z", L'7', ppszName);
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Mirror of CompressZipQuickCommand::GetIcon above.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_Compress7zQuick_Tooltip",
            L"Compress selected items to a 7-Zip archive (no dialog).");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(CompressSevenZQuickCommand);
        return S_OK;
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        return GetSharedState(items, pCmdState);
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"compress-7z");
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // ================================================================
    // CompressIndividuallyCommand — multi-select only.
    // ================================================================

    IFACEMETHODIMP CompressIndividuallyCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        // "각 항목별 압축(&U)" — Bandizip parity (shorter than its
        // `각각 파일명/폴더명으로 압축하기(U)`).
        std::wstring title = LoadShellString(
            L"Shell_CompressIndividually_Title",
            L"각 항목별 압축(&U)");
        return SHStrDupW(title.c_str(), ppszName);
    }
    IFACEMETHODIMP CompressIndividuallyCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }
    IFACEMETHODIMP CompressIndividuallyCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_CompressIndividually_Tooltip",
            L"Create one archive per selected item, using your default format.");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP CompressIndividuallyCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(CompressIndividuallyCommand);
        return S_OK;
    }
    IFACEMETHODIMP CompressIndividuallyCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        if (!pCmdState) return E_POINTER;
        if (!IsShellMenuEnabled())
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
        if (IsShellMenuNested())
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
        // Bandizip parity (spec §E): only hide when EVERY selected
        // item is an archive — mixed selections still get Compress.
        if (ItemsAreAllArchives(items))
        {
            *pCmdState = ECS_HIDDEN;
            return S_OK;
        }
        // Multi-select gate: hidden when only one item is selected.
        // For a single item the existing quick verbs already produce
        // the same archive (no point duplicating the row).
        DWORD count = 0;
        if (items && SUCCEEDED(items->GetCount(&count)) && count > 1)
        {
            *pCmdState = ECS_ENABLED;
        }
        else
        {
            *pCmdState = ECS_HIDDEN;
        }
        return S_OK;
    }
    IFACEMETHODIMP CompressIndividuallyCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"compress-individually");
    }
    IFACEMETHODIMP CompressIndividuallyCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP CompressIndividuallyCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }
}
