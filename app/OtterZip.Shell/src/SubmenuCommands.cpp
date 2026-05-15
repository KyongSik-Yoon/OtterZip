// OtterZip.Shell — submenu wrapper implementations + enumerator.
// See SubmenuCommands.h for the architecture rationale.

#include "SubmenuCommands.h"
#include "QuickCompressCommands.h"
#include "ShellInvoke.h"

#include <shlwapi.h>
#include <shobjidl.h>
#include <wrl/client.h>
#include <string>
#include <string_view>
#include <algorithm>

#pragma comment(lib, "Shlwapi.lib")

namespace OtterZip::Shell
{
    namespace
    {
        // Lower-case .ext suffixes we open. Mirrors the set declared in
        // Package.appxmanifest's FileTypeAssociation block — keep both
        // lists in sync if a new format lands.
        constexpr wchar_t const* kArchiveExts[] = {
            L".zip",  L".zipx", L".7z",   L".rar",  L".tar",
            L".tgz",  L".tbz",  L".tbz2", L".tlz",  L".txz",  L".tzst",
            L".gz",   L".bz2",  L".xz",   L".lzma", L".zst",  L".lz4",
            L".jar",  L".war",  L".ear",  L".ipa",  L".apk",  L".aab",
            L".xpi",  L".crx",  L".iso",  L".img",  L".cab",  L".deb",
        };

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

        // Returns true if *any* item in the array carries an archive
        // extension. Used to decide whether to surface "Extract here" in
        // the submenu — for a folder selection we just want Compress.
        bool ItemsContainAnyArchive(IShellItemArray* items) noexcept
        {
            if (!items) return false;
            DWORD count = 0;
            if (FAILED(items->GetCount(&count)) || count == 0) return false;

            for (DWORD i = 0; i < count; ++i)
            {
                Microsoft::WRL::ComPtr<IShellItem> item;
                if (FAILED(items->GetItemAt(i, item.GetAddressOf()))) continue;

                LPWSTR raw = nullptr;
                if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, &raw)) || !raw)
                {
                    continue;
                }
                std::wstring path{ raw };
                ::CoTaskMemFree(raw);

                if (HasArchiveExtension(path)) return true;
            }
            return false;
        }
    }

    // ----------------------------- SubmenuCompressCommand -----------------------------

    IFACEMETHODIMP SubmenuCompressCommand::GetTitle(IShellItemArray*, LPWSTR* ppszName) noexcept
    {
        return SHStrDupW(L"Compress with OtterZip", ppszName);
    }
    IFACEMETHODIMP SubmenuCompressCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP SubmenuCompressCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Create a new archive from the selected files.", ppszInfotip);
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
        wchar_t buf[MAX_PATH + 32] = {};
        ::swprintf_s(buf, L"%s.zip으로 압축(&Z)", stem.c_str());
        return SHStrDupW(buf, ppszName);
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP SubmenuCompressZipCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Compress selected items to a ZIP archive (no dialog).", ppszInfotip);
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
        wchar_t buf[MAX_PATH + 32] = {};
        ::swprintf_s(buf, L"%s.7z으로 압축(&7)", stem.c_str());
        return SHStrDupW(buf, ppszName);
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP SubmenuCompressSevenZCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Compress selected items to a 7-Zip archive (no dialog).", ppszInfotip);
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
        return SHStrDupW(L"Extract here", ppszName);
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::GetIcon(IShellItemArray*, LPWSTR* ppszIcon) noexcept
    {
        if (ppszIcon) *ppszIcon = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP SubmenuExtractHereCommand::GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) noexcept
    {
        return SHStrDupW(L"Extract this archive next to its current folder.", ppszInfotip);
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
