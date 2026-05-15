// OtterZip.Shell — Bandizip-style direct format-specific compress verbs.
//
// See QuickCompressCommands.h for the architecture rationale.

#include "QuickCompressCommands.h"
#include "ShellInvoke.h"
#include "ShellSettings.h"

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
    std::wstring DeriveSelectionStem(IShellItemArray* items) noexcept
    {
        if (items == nullptr)
        {
            return L"Archive";
        }
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
        // `SIGDN_PARENTRELATIVEFORADDRESSBAR` returns the name without
        // a leading drive/path — exactly what we want for the verb
        // label. Falls back to SIGDN_NORMALDISPLAY if the first call
        // fails (some virtual items).
        HRESULT hr = first->GetDisplayName(SIGDN_PARENTRELATIVEFORADDRESSBAR, &raw);
        if (FAILED(hr) || raw == nullptr)
        {
            hr = first->GetDisplayName(SIGDN_NORMALDISPLAY, &raw);
        }
        if (FAILED(hr) || raw == nullptr)
        {
            return L"Archive";
        }
        std::wstring name(raw);
        ::CoTaskMemFree(raw);

        // Strip the file extension so `foo.exe` → `foo` (the `.zip`
        // / `.7z` suffix is appended by the caller's title format).
        // Directory selections have no extension to strip — keep
        // the full name. `PathFindExtensionW` returns a pointer into
        // the input buffer, hence the offset arithmetic.
        wchar_t const* ext = ::PathFindExtensionW(name.c_str());
        if (ext != nullptr && *ext == L'.')
        {
            name.resize(static_cast<size_t>(ext - name.c_str()));
        }
        return name.empty() ? std::wstring(L"Archive") : name;
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

        // Compose `<basename>.<ext>으로 압축(&K)`. Hardcoded Korean
        // particle "으로" — matches Bandizip's universal usage
        // (avoids jongseong analysis; "으로" is grammatical with both
        // batchim-bearing and batchim-less stems). Hardcoded title
        // language pending full ShellStrings MUI integration.
        // Existing shell extension verbs already follow this hardcoded
        // pattern (`Compress with OtterZip` in CompressCommand.cpp:16);
        // matching the user's screenshot exactly here.
        HRESULT FormatTitle(
            IShellItemArray* items,
            wchar_t const* extWithDot,
            wchar_t hotkey,
            LPWSTR* ppszName) noexcept
        {
            std::wstring stem = DeriveSelectionStem(items);
            wchar_t buf[MAX_PATH + 32] = {};
            // Pattern: "<stem><ext>으로 압축(&<hotkey>)"
            //   ex: "DATA.zip으로 압축(&Z)"
            // Hardcoded Korean particle "으로" — matches Bandizip's
            // universal usage and avoids jongseong analysis. The
            // existing ExtractHereCommand / CompressCommand verbs in
            // this DLL are also hardcoded English; locale-aware
            // ShellStrings.cpp lands in a follow-up sprint.
            ::swprintf_s(buf, L"%s%s으로 압축(&%c)",
                         stem.c_str(), extWithDot, hotkey);
            return SHStrDupW(buf, ppszName);
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
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP CompressZipQuickCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Compress selected items to a ZIP archive (no dialog).", ppszInfotip);
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
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP CompressSevenZQuickCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Compress selected items to a 7-Zip archive (no dialog).", ppszInfotip);
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
}
