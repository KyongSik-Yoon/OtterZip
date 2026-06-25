// OtterZip.Shell — in-proc COM server entry points (Phase 7 PR-7D).
//
// Activation flow:
//   1. Explorer / COM Surrogate calls DllGetClassObject(rclsid, IID_IClassFactory, ...)
//   2. We dispatch on `rclsid` and hand back a per-CLSID ClassFactory<T>
//   3. Surrogate calls factory->CreateInstance(IID_IExplorerCommand, ...)
//   4. ClassFactory<T>::CreateInstance returns winrt::make<T>() — the
//      IExplorerCommand implementation in CompressCommand / ExtractHereCommand
//   5. Explorer drives GetTitle / GetIcon / GetState / Invoke as the user
//      hovers / clicks the menu item
//
// MSIX-managed registration: matching CLSID GUIDs are declared in
// `Package.appxmanifest` (com:Class Id="..."). Self-registration via
// regsvr32 is a no-op — installation is the package manifest, not the
// registry, in the packaged path.

#include "CompressCommand.h"
#include "ExtractCommands.h"
#include "ExtractHereCommand.h"
#include "OpenAppCommand.h"
#include "OtterzipMenuCommand.h"
#include "QuickCompressCommands.h"
#include "ShellAssets.h"
#include "ShellSettings.h"
#include "ShellStrings.h"

#include <windows.h>
#include <unknwn.h>
#include <combaseapi.h>
#include <mutex>
#include <winrt/base.h>

namespace
{
    std::atomic<long> g_dll_ref_count{ 0 };
    HMODULE g_module_handle = nullptr;
}

void OtterzipShell_AddRef() noexcept { ++g_dll_ref_count; }
void OtterzipShell_Release() noexcept { --g_dll_ref_count; }

// Expose the DLL's own module handle so ShellAssets can resolve packaged
// asset paths from the DLL's install directory (GetModuleFileNameW) —
// a pure-Win32 path that doesn't establish the AppX activation context,
// unlike Package.Current.InstalledLocation.
extern "C" HMODULE OtterzipShell_GetModuleHandle() noexcept { return g_module_handle; }

extern "C" BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID /*reserved*/) noexcept
{
    switch (reason)
    {
        case DLL_PROCESS_ATTACH:
            DisableThreadLibraryCalls(module);
            g_module_handle = module;
            break;
        case DLL_PROCESS_DETACH:
            g_module_handle = nullptr;
            break;
        default:
            break;
    }
    return TRUE;
}

namespace OtterZip::Shell
{
    // Generic per-CLSID class factory. We need one factory instance for
    // each verb's CLSID since the factory itself is what `DllGetClassObject`
    // hands back; CreateInstance just spins up the bound IExplorerCommand
    // implementation.
    template <typename T>
    struct ClassFactory : winrt::implements<ClassFactory<T>, IClassFactory>
    {
        IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** ppv) noexcept override
        {
            if (ppv == nullptr)
            {
                return E_POINTER;
            }
            *ppv = nullptr;
            // COM aggregation isn't useful for IExplorerCommand verbs and
            // refusing it removes a class of edge-case crashes.
            if (outer != nullptr)
            {
                return CLASS_E_NOAGGREGATION;
            }
            try
            {
                auto instance = winrt::make<T>();
                return instance->QueryInterface(riid, ppv);
            }
            catch (winrt::hresult_error const& e)
            {
                return e.code();
            }
            catch (...)
            {
                return E_FAIL;
            }
        }

        IFACEMETHODIMP LockServer(BOOL lock) noexcept override
        {
            // Surrogate uses LockServer to keep the DLL pinned in memory
            // even when no objects are alive. Mirror that into the same
            // ref count `DllCanUnloadNow` checks.
            if (lock)
            {
                ::OtterzipShell_AddRef();
            }
            else
            {
                ::OtterzipShell_Release();
            }
            return S_OK;
        }
    };
}

// SDK declares DllCanUnloadNow / DllGetClassObject in <combaseapi.h> WITHOUT
// the C++ `noexcept` specifier. Repeating it here is a redefinition error
// (C2382) on MSVC. We keep the bodies non-throwing in practice; the
// signature must match the SDK exactly.
extern "C" HRESULT __stdcall DllCanUnloadNow()
{
    return g_dll_ref_count.load() == 0 ? S_OK : S_FALSE;
}

namespace OtterZip::Shell
{
    // Prime every cache the verbs hit on their first hover. Without
    // this, each lazy fetch (LocalSettings RPC, Package.InstalledLocation,
    // ResourceLoader.GetForViewIndependentUse) costs 50-200ms cold and
    // the cumulative GetState time pushes past Windows 11's Modern
    // Context Menu timeout — Explorer renders the right-click without
    // our verbs and the user has to right-click again to see them.
    //
    // Called synchronously inside the FIRST DllGetClassObject (which
    // COM invokes before our class instance is even created). That's
    // outside DllMain so all the WinRT-touching code is legal here,
    // and the warmup completes before the GetState call that needs
    // the primed caches.
    static void WarmupCachesOnce() noexcept
    {
        static std::once_flag s_flag;
        std::call_once(s_flag, []() {
            try
            {
                // Settings cache — fetches both LocalSettings flags
                // we read on every GetState.
                (void)IsShellMenuEnabled();
                (void)IsShellMenuNested();

                // Package install location — used by GetIcon path
                // resolution. First fetch crosses the AppX runtime
                // boundary.
                (void)GetPackagedAssetPath(L"Assets\\AppIcon.ico");

                // ResourceLoader — first GetForViewIndependentUse
                // call binds the package's PRI map. Probe with a
                // real key that all verbs share.
                (void)LoadShellString(L"Shell_CompressDialog_Title", L"");
            }
            catch (...)
            {
                // Warmup failures are non-fatal — the real GetState
                // calls will fall back to the same defaults that
                // would have applied without the warmup.
            }
        });
    }
}

extern "C" HRESULT __stdcall DllGetClassObject(REFCLSID rclsid, REFIID riid, LPVOID* ppv)
{
    if (ppv == nullptr)
    {
        return E_POINTER;
    }
    *ppv = nullptr;

    using namespace OtterZip::Shell;

    // Synchronous first-time cache warmup — see WarmupCachesOnce
    // rationale. After the first invocation `call_once` is a single
    // atomic load, so we pay no measurable cost on subsequent calls.
    WarmupCachesOnce();

    if (rclsid == __uuidof(CompressCommand))
    {
        try
        {
            auto factory = winrt::make<ClassFactory<CompressCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(ExtractHereCommand))
    {
        try
        {
            auto factory = winrt::make<ClassFactory<ExtractHereCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(OtterzipMenuCommand))
    {
        // Phase 9 nested-menu wiring: third class object surfaces the
        // "OtterZip" parent verb that hosts a Compress/Extract submenu.
        // Active when Settings_ShellMenuMode == "nested" (default).
        try
        {
            auto factory = winrt::make<ClassFactory<OtterzipMenuCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(CompressZipQuickCommand))
    {
        // Bandizip-style direct ZIP quick-compress verb.
        try
        {
            auto factory = winrt::make<ClassFactory<CompressZipQuickCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(CompressSevenZQuickCommand))
    {
        // Bandizip-style direct 7z quick-compress verb.
        try
        {
            auto factory = winrt::make<ClassFactory<CompressSevenZQuickCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    // ----------------------------------------------------------------
    // 4 new verbs landed 2026-05-19 (Bandizip 4-context parity sprint).
    // ExtractSmart / ExtractToSubfolder / ExtractDialog handle archive
    // selections; CompressIndividually handles multi-select compress.
    // ----------------------------------------------------------------
    if (rclsid == __uuidof(ExtractSmartCommand))
    {
        try
        {
            auto factory = winrt::make<ClassFactory<ExtractSmartCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(ExtractToSubfolderCommand))
    {
        try
        {
            auto factory = winrt::make<ClassFactory<ExtractToSubfolderCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(ExtractDialogCommand))
    {
        try
        {
            auto factory = winrt::make<ClassFactory<ExtractDialogCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(CompressIndividuallyCommand))
    {
        try
        {
            auto factory = winrt::make<ClassFactory<CompressIndividuallyCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    if (rclsid == __uuidof(OpenAppCommand))
    {
        // 2026-06-18: empty-space (Directory\Background) "OtterZip 열기" verb.
        try
        {
            auto factory = winrt::make<ClassFactory<OpenAppCommand>>();
            return factory->QueryInterface(riid, ppv);
        }
        catch (winrt::hresult_error const& e) { return e.code(); }
        catch (...) { return E_FAIL; }
    }
    return CLASS_E_CLASSNOTAVAILABLE;
}

extern "C" HRESULT __stdcall DllRegisterServer()
{
    // MSIX-managed registration in production; stub returns S_OK so the
    // tooling treats the build as valid. Per-user regsvr32 install is
    // not supported (matches `docs/03-api/shell-extension.md` §9).
    return S_OK;
}

extern "C" HRESULT __stdcall DllUnregisterServer()
{
    return S_OK;
}
