// OtterZip.Shell — implementation of GetPackagedAssetPath.
//
// The install root is fetched exactly once (under a mutex) and reused
// across every verb hover. Explorer fires `GetIcon` on every right-
// click, so this cache matters — `Package.Current.InstalledLocation`
// is not free (RPC into AppX runtime).

#include "ShellAssets.h"

#include <mutex>
#include <winrt/Windows.ApplicationModel.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Storage.h>

namespace OtterZip::Shell
{
    namespace
    {
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
