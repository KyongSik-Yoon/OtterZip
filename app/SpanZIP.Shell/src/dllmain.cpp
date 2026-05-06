// SpanZIP.Shell — in-proc COM server entry points (Phase 7 PR-7D).
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
#include "ExtractHereCommand.h"

#include <windows.h>
#include <unknwn.h>
#include <combaseapi.h>
#include <winrt/base.h>

namespace
{
    std::atomic<long> g_dll_ref_count{ 0 };
    HMODULE g_module_handle = nullptr;
}

void SpanzipShell_AddRef() noexcept { ++g_dll_ref_count; }
void SpanzipShell_Release() noexcept { --g_dll_ref_count; }

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

namespace SpanZIP::Shell
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
                ::SpanzipShell_AddRef();
            }
            else
            {
                ::SpanzipShell_Release();
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

extern "C" HRESULT __stdcall DllGetClassObject(REFCLSID rclsid, REFIID riid, LPVOID* ppv)
{
    if (ppv == nullptr)
    {
        return E_POINTER;
    }
    *ppv = nullptr;

    using namespace SpanZIP::Shell;

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
