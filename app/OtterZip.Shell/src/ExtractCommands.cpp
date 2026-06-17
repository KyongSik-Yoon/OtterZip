// OtterZip.Shell — extract verb implementations. See ExtractCommands.h.

#include "ExtractCommands.h"

#include "ArchiveDetection.h"
#include "ShellAssets.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"
#include "ShellStrings.h"

#include <pathcch.h>
#include <shellapi.h>
#include <shlwapi.h>
#include <string>
#include <wrl/client.h>

#pragma comment(lib, "Shlwapi.lib")
#pragma comment(lib, "Pathcch.lib")

namespace OtterZip::Shell
{
    namespace
    {
        // Number of selected items (0 on any failure). Used to switch
        // verb labels between the single-archive form
        // (`<stem>\에 풀기`) and the multi-select "each" form
        // (`각각 파일명 폴더에 풀기`) — Bandizip parity (2026-06-17).
        DWORD SelectionCount(IShellItemArray* items) noexcept
        {
            DWORD count = 0;
            if (items == nullptr || FAILED(items->GetCount(&count)))
            {
                return 0;
            }
            return count;
        }

        // Shared by ExtractToSubfolderCommand::GetTitle for the
        // dynamic `<stem>\에 풀기` label. Mirrors DeriveSelectionStem
        // from QuickCompressCommands but strips ALL extensions
        // including compound ones (`.tar.gz` → `archive`).
        std::wstring DeriveArchiveStem(IShellItemArray* items) noexcept
        {
            if (items == nullptr) return L"Archive";
            DWORD count = 0;
            if (FAILED(items->GetCount(&count)) || count == 0)
            {
                return L"Archive";
            }
            Microsoft::WRL::ComPtr<IShellItem> first;
            if (FAILED(items->GetItemAt(0, first.GetAddressOf())) || first == nullptr)
            {
                return L"Archive";
            }
            LPWSTR raw = nullptr;
            HRESULT hr = first->GetDisplayName(SIGDN_PARENTRELATIVEFORADDRESSBAR, &raw);
            if (FAILED(hr) || raw == nullptr) return L"Archive";
            std::wstring name(raw);
            ::CoTaskMemFree(raw);

            // Strip compound extensions for the common cases. The
            // ordering matters — `.tar.gz` needs `.gz` peeled first
            // then `.tar`. We keep the list small (6 compound exts)
            // and do single-pass for everything else.
            constexpr wchar_t const* kCompoundOuter[] = {
                L".gz", L".bz2", L".xz", L".zst", L".lz4", L".lzma",
            };
            for (auto suffix : kCompoundOuter)
            {
                size_t slen = ::wcslen(suffix);
                if (name.size() > slen)
                {
                    size_t at = name.size() - slen;
                    if (::_wcsicmp(name.c_str() + at, suffix) == 0)
                    {
                        name.resize(at);
                        break;
                    }
                }
            }
            // Now strip single trailing extension (handles .tar after
            // .gz peel above, and .zip / .7z / .rar etc.).
            wchar_t const* ext = ::PathFindExtensionW(name.c_str());
            if (ext != nullptr && *ext == L'.')
            {
                name.resize(static_cast<size_t>(ext - name.c_str()));
            }
            return name.empty() ? std::wstring(L"Archive") : name;
        }

        // Common gating: master toggle + at-least-one-archive
        // selection. Used by all three extract verbs.
        HRESULT GetSharedExtractState(IShellItemArray* items, EXPCMDSTATE* pCmdState) noexcept
        {
            if (pCmdState == nullptr) return E_POINTER;
            if (!IsShellMenuEnabled())
            {
                *pCmdState = ECS_HIDDEN;
                return S_OK;
            }
            if (IsShellMenuNested())
            {
                // Nested mode hides leaf extract verbs at top level;
                // the parent submenu provides its own wrappers
                // (analogue to SubmenuExtractHereCommand). Keeps the
                // flat / nested layouts mutually exclusive.
                *pCmdState = ECS_HIDDEN;
                return S_OK;
            }
            // Bandizip parity (spec §E): hide Extract verbs on mixed
            // selections (some archives, some non-archives). The
            // manifest `.zip` / `.7z` ItemType still attaches us to
            // any selection touching an archive, so the GetState
            // filter is what enforces archive-only visibility.
            // Single-archive selection: ItemsAreAllArchives is true.
            // Mixed: false → Compress verbs alone render.
            if (!ItemsAreAllArchives(items))
            {
                *pCmdState = ECS_HIDDEN;
                return S_OK;
            }
            *pCmdState = ECS_ENABLED;
            return S_OK;
        }

        // Shared icon lookup — every extract verb shows the brand mark.
        HRESULT FillBrandIcon(LPWSTR* ppszIcon) noexcept
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
    }

    // ============================ ExtractSmart =====================

    IFACEMETHODIMP ExtractSmartCommand::GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept
    {
        // Single: "알아서 풀기(&Z)". Multi: "각각 알아서 풀기(&Z)" —
        // each archive decides its own layout independently, so the
        // label says "each" (Bandizip 각각 알아서 풀기 parity). Without
        // this the multi-select label wrongly implied one combined op.
        std::wstring title = (SelectionCount(items) > 1)
            ? LoadShellString(L"Shell_ExtractSmartEach_Title", L"각각 알아서 풀기(&Z)")
            : LoadShellString(L"Shell_ExtractSmart_Title", L"알아서 풀기(&Z)");
        return SHStrDupW(title.c_str(), ppszName);
    }
    IFACEMETHODIMP ExtractSmartCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        return FillBrandIcon(ppszIcon);
    }
    IFACEMETHODIMP ExtractSmartCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_ExtractSmart_Tooltip",
            L"Auto-detect single-root layout and extract accordingly.");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP ExtractSmartCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(ExtractSmartCommand);
        return S_OK;
    }
    IFACEMETHODIMP ExtractSmartCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        return GetSharedExtractState(items, pCmdState);
    }
    IFACEMETHODIMP ExtractSmartCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"extract-smart");
    }
    IFACEMETHODIMP ExtractSmartCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP ExtractSmartCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // ======================== ExtractToSubfolder ===================

    IFACEMETHODIMP ExtractToSubfolderCommand::GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept
    {
        // Multi-select: "각각 파일명 폴더에 풀기(&E)" — each archive
        // extracts into ITS OWN <stem>/ folder (ExtractEachAsync runs
        // per-path with its own stem). The single-archive `<firstname>\
        // 에 풀기` label was misleading on multi-select because it named
        // only the first archive while all of them get their own folder.
        // Bandizip uses "각각 파일명 폴더에 풀기" for exactly this.
        if (SelectionCount(items) > 1)
        {
            std::wstring each = LoadShellString(
                L"Shell_ExtractToSubfolderEach_Title",
                L"각각 파일명 폴더에 풀기(&E)");
            return SHStrDupW(each.c_str(), ppszName);
        }
        // Single: "<stem>\에 풀기(&E)" — locale-aware format with `{0}`
        // = archive stem. Korean default mirrors Bandizip.
        std::wstring stem = DeriveArchiveStem(items);
        std::wstring fmt = LoadShellString(
            L"Shell_ExtractToSubfolder_TitleFormat",
            L"{0}\\에 풀기(&E)");
        // Same substitution helper shape as QuickCompressCommands.
        size_t pos = fmt.find(L"{0}");
        if (pos != std::wstring::npos)
        {
            fmt.replace(pos, 3, stem);
        }
        return SHStrDupW(fmt.c_str(), ppszName);
    }
    IFACEMETHODIMP ExtractToSubfolderCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        return FillBrandIcon(ppszIcon);
    }
    IFACEMETHODIMP ExtractToSubfolderCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_ExtractToSubfolder_Tooltip",
            L"Extract into a new folder named after the archive.");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP ExtractToSubfolderCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(ExtractToSubfolderCommand);
        return S_OK;
    }
    IFACEMETHODIMP ExtractToSubfolderCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        return GetSharedExtractState(items, pCmdState);
    }
    IFACEMETHODIMP ExtractToSubfolderCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"extract-to-subfolder");
    }
    IFACEMETHODIMP ExtractToSubfolderCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP ExtractToSubfolderCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // ========================== ExtractDialog ======================

    IFACEMETHODIMP ExtractDialogCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        // "OtterZip으로 풀기...(&B)" — opens the standalone
        // ExtractOptionsDialog window for destination / overwrite /
        // password selection. The `&B` hotkey mirrors Bandizip's
        // `반디집으로 압축 풀기(&B)`.
        std::wstring title = LoadShellString(
            L"Shell_ExtractDialog_Title",
            L"OtterZip으로 풀기...(&B)");
        return SHStrDupW(title.c_str(), ppszName);
    }
    IFACEMETHODIMP ExtractDialogCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        return FillBrandIcon(ppszIcon);
    }
    IFACEMETHODIMP ExtractDialogCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_ExtractDialog_Tooltip",
            L"Open the OtterZip extract dialog (choose destination, overwrite policy, password).");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP ExtractDialogCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = __uuidof(ExtractDialogCommand);
        return S_OK;
    }
    IFACEMETHODIMP ExtractDialogCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        return GetSharedExtractState(items, pCmdState);
    }
    IFACEMETHODIMP ExtractDialogCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"extract-dialog");
    }
    IFACEMETHODIMP ExtractDialogCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP ExtractDialogCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }
}
