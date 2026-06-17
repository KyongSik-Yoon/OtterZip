// OtterZip.Shell — submenu wrapper implementations + enumerator.
// See SubmenuCommands.h for the architecture rationale.

#include "SubmenuCommands.h"
#include "ArchiveDetection.h"
#include "QuickCompressCommands.h"
#include "ShellAssets.h"
#include "ShellInvoke.h"
#include "ShellStrings.h"

#include <shlwapi.h>
#include <shobjidl.h>
#include <wrl/client.h>
#include <string>
#include <string_view>
#include <algorithm>

#pragma comment(lib, "Shlwapi.lib")

namespace OtterZip::Shell::detail
{
    // Shared placeholder substitution helper, matches the FormatTitle
    // logic in QuickCompressCommands. Used by the two submenu quick
    // verb classes below.
    static std::wstring SubstituteFmt(std::wstring fmt, std::wstring const& a,
                                      std::wstring const& b, std::wstring const& c)
    {
        auto replace = [](std::wstring& s, std::wstring const& token, std::wstring const& v)
        {
            size_t pos = s.find(token);
            if (pos != std::wstring::npos)
            {
                s.replace(pos, token.size(), v);
            }
        };
        replace(fmt, L"{0}", a);
        replace(fmt, L"{1}", b);
        replace(fmt, L"{2}", c);
        return fmt;
    }
}

namespace OtterZip::Shell
{
    // Archive-detection helpers (kArchiveExts, HasArchiveExtension,
    // ItemsContainAnyArchive) moved to ArchiveDetection.{h,cpp} so the
    // Compress* GetStates in QuickCompressCommands.cpp / CompressCommand.cpp
    // can share the same logic for Bandizip parity (archive context →
    // Compress verbs hidden).

    // ----------------------------- SubmenuCompressCommand -----------------------------

    IFACEMETHODIMP SubmenuCompressCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        // Reuse the same key as the top-level CompressCommand so the
        // submenu label is consistent with the flat-mode equivalent.
        std::wstring title = LoadShellString(
            L"Shell_CompressDialog_Title",
            L"OtterZip으로 압축...(&O)");
        return SHStrDupW(title.c_str(), ppszName);
    }
    IFACEMETHODIMP SubmenuCompressCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Submenu wrappers share the brand icon with their top-level
        // siblings — nested-mode users see consistent iconography
        // inside the "OtterZip ▶" parent menu. Identical resolution
        // pattern as CompressCommand::GetIcon etc.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }
    IFACEMETHODIMP SubmenuCompressCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_CompressDialog_Tooltip",
            L"Create a new archive from the selected files.");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP SubmenuCompressCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        // Borrow the canonical CompressCommand CLSID — ensures Explorer
        // dedupes / treats clicks here the same way it treats the
        // top-level Compress verb.
        *pguidCommandName = { 0x44444444, 0x2222, 0x3333, { 0x44, 0x44, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55 } };
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressCommand::GetState(IShellItemArray*, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        // Inside the submenu we trust the parent's gating already
        // happened — just enable. Compress works on any file/folder.
        if (pCmdState) *pCmdState = ECS_ENABLED;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"compress");
    }
    IFACEMETHODIMP SubmenuCompressCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // ---------------------------- SubmenuCompressZipCommand --------------------------

    IFACEMETHODIMP SubmenuCompressZipCommand::GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept
    {
        std::wstring stem = DeriveSelectionStem(items);
        std::wstring fmt = LoadShellString(
            L"Shell_CompressQuick_TitleFormat",
            L"{0}{1}으로 압축(&{2})");
        std::wstring out = detail::SubstituteFmt(fmt, stem, L".zip", L"Z");
        return SHStrDupW(out.c_str(), ppszName);
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Submenu wrappers share the brand icon with their top-level
        // siblings — nested-mode users see consistent iconography
        // inside the "OtterZip ▶" parent menu. Identical resolution
        // pattern as CompressCommand::GetIcon etc.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_CompressZipQuick_Tooltip",
            L"Compress selected items to a ZIP archive (no dialog).");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        // Same CLSID as the top-level CompressZipQuickCommand so
        // Explorer dedupes the two surfaces. The submenu version
        // ignores the flat/nested gate (parent menu already filtered).
        *pguidCommandName = { 0x55555555, 0x2222, 0x3333,
                              { 0x44, 0x44, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55 } };
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::GetState(IShellItemArray*, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        if (pCmdState) *pCmdState = ECS_ENABLED;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"compress-zip");
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // ---------------------------- SubmenuCompressSevenZCommand -----------------------

    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetTitle(IShellItemArray* items, LPWSTR* ppszName) noexcept
    {
        std::wstring stem = DeriveSelectionStem(items);
        std::wstring fmt = LoadShellString(
            L"Shell_CompressQuick_TitleFormat",
            L"{0}{1}으로 압축(&{2})");
        std::wstring out = detail::SubstituteFmt(fmt, stem, L".7z", L"7");
        return SHStrDupW(out.c_str(), ppszName);
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Submenu wrappers share the brand icon with their top-level
        // siblings — nested-mode users see consistent iconography
        // inside the "OtterZip ▶" parent menu. Identical resolution
        // pattern as CompressCommand::GetIcon etc.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_Compress7zQuick_Tooltip",
            L"Compress selected items to a 7-Zip archive (no dialog).");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = { 0x66666666, 0x2222, 0x3333,
                              { 0x44, 0x44, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55 } };
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetState(IShellItemArray*, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        if (pCmdState) *pCmdState = ECS_ENABLED;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"compress-7z");
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // ---------------------------- SubmenuExtractHereCommand ---------------------------

    IFACEMETHODIMP SubmenuExtractHereCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        // Inside the parent submenu the verb doesn't need the brand
        // suffix — Explorer already shows the parent's "OtterZip" label
        // above. Distinct key from the top-level Shell_ExtractHere_Title.
        std::wstring title = LoadShellString(L"Shell_ExtractHereSubmenu_Title", L"Extract here");
        return SHStrDupW(title.c_str(), ppszName);
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        // Submenu wrappers share the brand icon with their top-level
        // siblings — nested-mode users see consistent iconography
        // inside the "OtterZip ▶" parent menu. Identical resolution
        // pattern as CompressCommand::GetIcon etc.
        if (!ppszIcon) return E_POINTER;
        std::wstring path = GetPackagedAssetPath(L"Assets\\AppIcon.ico");
        if (path.empty())
        {
            *ppszIcon = nullptr;
            return E_NOTIMPL;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        std::wstring tooltip = LoadShellString(
            L"Shell_ExtractHere_Tooltip",
            L"Extract this archive next to its current folder.");
        return SHStrDupW(tooltip.c_str(), ppszInfotip);
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::GetCanonicalName(GUID* pguidCommandName) noexcept
    {
        if (!pguidCommandName) return E_POINTER;
        *pguidCommandName = { 0x22222222, 0x2222, 0x3333, { 0x44, 0x44, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55 } };
        return S_OK;
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* pCmdState) noexcept
    {
        if (!pCmdState) return E_POINTER;
        // Hide when the selection has no archive extensions — keeps the
        // submenu uncluttered for folder / non-archive selections.
        *pCmdState = ItemsContainAnyArchive(items) ? ECS_ENABLED : ECS_HIDDEN;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::Invoke(IShellItemArray* items, IBindCtx*) noexcept
    {
        return InvokeHostApp(items, L"extract-here");
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::GetFlags(EXPCMDFLAGS* pFlags) noexcept
    {
        if (!pFlags) return E_POINTER;
        *pFlags = ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::EnumSubCommands(IEnumExplorerCommand** ppEnum) noexcept
    {
        if (ppEnum) *ppEnum = nullptr;
        return E_NOTIMPL;
    }

    // -------------------------------- SubmenuEnumerator -------------------------------

    IFACEMETHODIMP SubmenuEnumerator::Next(ULONG celt, IExplorerCommand** apUICommand, ULONG* pceltFetched) noexcept
    {
        if (!apUICommand) return E_POINTER;
        if (pceltFetched) *pceltFetched = 0;

        ULONG produced = 0;
        try
        {
            while (produced < celt && m_index < kCount)
            {
                winrt::com_ptr<IExplorerCommand> child;
                switch (m_index)
                {
                    case 0:
                    {
                        // Bandizip-style direct ZIP quick verb first —
                        // matches the top-level flat layout order.
                        auto cmd = winrt::make<SubmenuCompressZipCommand>();
                        child.copy_from(cmd.as<IExplorerCommand>().get());
                        break;
                    }
                    case 1:
                    {
                        auto cmd = winrt::make<SubmenuCompressSevenZCommand>();
                        child.copy_from(cmd.as<IExplorerCommand>().get());
                        break;
                    }
                    case 2:
                    {
                        // "OtterZip으로 압축..." — opens MainWindow.
                        auto cmd = winrt::make<SubmenuCompressCommand>();
                        child.copy_from(cmd.as<IExplorerCommand>().get());
                        break;
                    }
                    default:
                    {
                        // m_index == 3 — Extract here (visible only
                        // on archive selections; its own GetState
                        // does the extension probe).
                        auto cmd = winrt::make<SubmenuExtractHereCommand>();
                        child.copy_from(cmd.as<IExplorerCommand>().get());
                        break;
                    }
                }
                apUICommand[produced] = child.detach();
                ++produced;
                ++m_index;
            }
        }
        catch (winrt::hresult_error const& e)
        {
            if (pceltFetched) *pceltFetched = produced;
            return e.code();
        }
        catch (...)
        {
            if (pceltFetched) *pceltFetched = produced;
            return E_FAIL;
        }

        if (pceltFetched) *pceltFetched = produced;
        return (produced == celt) ? S_OK : S_FALSE;
    }

    IFACEMETHODIMP SubmenuEnumerator::Skip(ULONG celt) noexcept
    {
        if (m_index + celt > kCount)
        {
            m_index = kCount;
            return S_FALSE;
        }
        m_index += celt;
        return S_OK;
    }

    IFACEMETHODIMP SubmenuEnumerator::Reset() noexcept
    {
        m_index = 0;
        return S_OK;
    }

    IFACEMETHODIMP SubmenuEnumerator::Clone(IEnumExplorerCommand** ppenum) noexcept
    {
        if (!ppenum) return E_POINTER;
        *ppenum = nullptr;
        try
        {
            auto clone = winrt::make_self<SubmenuEnumerator>();
            clone->m_index = m_index;
            return clone.as<IEnumExplorerCommand>()->QueryInterface(IID_PPV_ARGS(ppenum));
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
}
