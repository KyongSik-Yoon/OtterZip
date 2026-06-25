// OtterZip.Shell — implementation of GetPackagedAssetPath.
//
// The install root is fetched exactly once (under a mutex) and reused
// across every verb hover. Explorer fires `GetIcon` on every right-
// click, so this cache matters — `Package.Current.InstalledLocation`
// is not free (RPC into AppX runtime).

#include "ShellAssets.h"

#include <windows.h>
#include <mutex>
#include <string>
#include <winrt/Windows.ApplicationModel.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Storage.h>

// Defined in dllmain.cpp — the shell DLL's own module handle.
extern "C" HMODULE OtterzipShell_GetModuleHandle() noexcept;

namespace OtterZip::Shell
{
    namespace
    {
        // Resolve a packaged asset from the DLL's own install directory
        // using GetModuleFileNameW — a pure-Win32 call that does NOT
        // establish the AppX activation context (unlike
        // Package.Current.InstalledLocation). The shell DLL ships at the
        // package root next to Assets\, so <dll dir>\<relative> is the
        // asset. Returns empty if the module path can't be resolved or the
        // file isn't there, so GetPackagedAssetPath falls back to WinRT.
        std::wstring ModuleRelativeAssetPath(wchar_t const* relative) noexcept
        {
            HMODULE mod = OtterzipShell_GetModuleHandle();
            if (mod == nullptr)
            {
                return {};
            }
            wchar_t buf[1024];
            DWORD n = GetModuleFileNameW(mod, buf, 1024);
            if (n == 0 || n >= 1024)
            {
                return {};
            }
            std::wstring path(buf, n);
            size_t slash = path.find_last_of(L"\\/");
            if (slash == std::wstring::npos)
            {
                return {};
            }
            std::wstring out = path.substr(0, slash + 1);
            if (relative[0] == L'\\' || relative[0] == L'/')
            {
                ++relative;
            }
            out += relative;
            if (GetFileAttributesW(out.c_str()) == INVALID_FILE_ATTRIBUTES)
            {
                return {};
            }
            return out;
        }

        std::mutex g_rootMu;
        // Cached `Package.Current.InstalledLocation.Path()`. Empty until
        // first successful fetch; an empty value also represents a
        // permanent failure (we don't retry — if the surrogate has no
        // package identity it never will gain one during this lifetime).
        std::wstring g_packageRoot;
        bool g_rootResolved = false;

        std::wstring const& EnsurePackageRoot() noexcept
        {
            std::lock_guard<std::mutex> lock(g_rootMu);
            if (g_rootResolved)
            {
                return g_packageRoot;
            }
            try
            {
                auto pkg = winrt::Windows::ApplicationModel::Package::Current();
                auto folder = pkg.InstalledLocation();
                winrt::hstring path = folder.Path();
                g_packageRoot.assign(path.c_str());
            }
            catch (...)
            {
                // Leave g_packageRoot empty — callers fall back to E_NOTIMPL.
                g_packageRoot.clear();
            }
            g_rootResolved = true;
            return g_packageRoot;
        }
    }

    std::wstring GetPackagedAssetPath(wchar_t const* relative) noexcept
    {
        if (relative == nullptr || *relative == L'\0')
        {
            return {};
        }
        // Fast path: derive from the DLL's own directory (no AppX context).
        try
        {
            std::wstring fast = ModuleRelativeAssetPath(relative);
            if (!fast.empty())
            {
                return fast;
            }
        }
        catch (...)
        {
            // fall through to the WinRT path below
        }
        std::wstring const& root = EnsurePackageRoot();
        if (root.empty())
        {
            return {};
        }
        std::wstring out;
        out.reserve(root.size() + 1 + ::wcslen(relative));
        out.append(root);
        // Always insert one separator. `relative` may start with '\' or
        // '/' — strip the leading separator so we don't emit a double.
        if (relative[0] == L'\\' || relative[0] == L'/')
        {
            ++relative;
        }
        out.push_back(L'\\');
        out.append(relative);
        return out;
    }
}
