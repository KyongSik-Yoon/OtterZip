// OtterZip.Shell — host app launcher.

#include "ShellInvoke.h"

#include <shellapi.h>
#include <shlwapi.h>
#include <pathcch.h>
#include <combaseapi.h>

// `<pathcch.h>` declares PathCchCombine / PathCchRemoveFileSpec but the
// auto-link pragma lives in Pathcch.lib only — explicit link required.
#pragma comment(lib, "Pathcch.lib")

namespace OtterZip::Shell
{
    namespace
    {
        // Returns the directory containing this DLL — host EXE is bundled
        // alongside in MSIX packaging, and lives next to the build output
        // for unpackaged dev runs.
        std::wstring DllDirectory() noexcept
        {
            HMODULE module = nullptr;
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                reinterpret_cast<LPCWSTR>(&DllDirectory),
                &module);
            wchar_t buf[MAX_PATH] = {};
            DWORD len = GetModuleFileNameW(module, buf, MAX_PATH);
            if (len == 0 || len == MAX_PATH)
            {
                return {};
            }
            // Strip the file name to leave the directory.
            PathCchRemoveFileSpec(buf, MAX_PATH);
            return std::wstring(buf);
        }

        // Builds `"<p1>;<p2>;..."` (with surrounding quotes) from the
        // IShellItemArray. NTFS paths cannot contain `"` or `;` (both
        // are illegal filename chars on NTFS), so naive concatenation
        // is safe — we still defensively skip any item whose display
        // name somehow contains either character to keep argv parsing
        // unambiguous on the App side (see App.ParseInvokeArgs).
        std::wstring BuildFilesArg(IShellItemArray* items) noexcept
        {
            std::wstring out;
            out.reserve(MAX_PATH);
            out.push_back(L'"');

            DWORD count = 0;
            if (!items || FAILED(items->GetCount(&count)) || count == 0)
            {
                out.push_back(L'"');
                return out;
            }
            bool first = true;
            for (DWORD i = 0; i < count; ++i)
            {
                IShellItem* item = nullptr;
                if (FAILED(items->GetItemAt(i, &item)) || !item)
                {
                    continue;
                }
                LPWSTR pszPath = nullptr;
                HRESULT hr = item->GetDisplayName(SIGDN_FILESYSPATH, &pszPath);
                if (SUCCEEDED(hr) && pszPath)
                {
                    std::wstring p(pszPath);
                    CoTaskMemFree(pszPath);
                    // Defensive: skip pathologic display names rather
                    // than smuggle them into argv. Real NTFS paths
                    // never trigger this branch.
                    bool unsafe = (p.find(L'"') != std::wstring::npos)
                               || (p.find(L';') != std::wstring::npos);
                    if (!unsafe)
                    {
                        if (!first)
                        {
                            out.push_back(L';');
                        }
                        out.append(p);
                        first = false;
                    }
                }
                item->Release();
            }
            out.push_back(L'"');
            return out;
        }
    }

    HRESULT InvokeHostApp(IShellItemArray* items, std::wstring_view verb) noexcept
    {
        std::wstring exePath = DllDirectory();
        if (exePath.empty())
        {
            return E_FAIL;
        }
        // Append `\OtterZip.exe` — `PathCchCombine` is the modern Path API.
        wchar_t fullExe[MAX_PATH] = {};
        if (FAILED(PathCchCombine(fullExe, MAX_PATH, exePath.c_str(), L"OtterZip.exe")))
        {
            return E_FAIL;
        }

        std::wstring args;
        args.append(L"--invoke ");
        args.append(verb);
        args.append(L" --files ");
        args.append(BuildFilesArg(items));

        SHELLEXECUTEINFOW info{};
        info.cbSize = sizeof(info);
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = L"open";
        info.lpFile = fullExe;
        info.lpParameters = args.c_str();
        info.nShow = SW_SHOWNORMAL;
        if (!ShellExecuteExW(&info))
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }
        if (info.hProcess)
        {
            CloseHandle(info.hProcess);
        }
        return S_OK;
    }
}
