// OtterZip.Shell — implementation of the Win32-only LocalState cache reader.
// See ShellCache.h for the rationale.

#include "ShellCache.h"

#include <windows.h>
#include <appmodel.h>   // GetCurrentPackageFamilyName
#include <mutex>
#include <string>
#include <unordered_map>

namespace OtterZip::Shell
{
    namespace
    {
        constexpr long long kMaxCacheBytes = 1 << 20;   // 1 MiB sanity cap

        std::mutex g_mu;
        std::wstring g_cachePath;
        bool g_pathResolved = false;
        bool g_pathValid = false;
        FILETIME g_lastWrite{};
        bool g_haveData = false;
        std::unordered_map<std::wstring, std::wstring> g_map;

        // Build %LOCALAPPDATA%\Packages\<PFN>\LocalState\shell-menu-cache.v1.txt
        // entirely with fast Win32 calls. GetCurrentPackageFamilyName reads
        // the process token's package identity — no AppX activation context,
        // no cross-process RPC. Returns empty when there is no package
        // identity (unpackaged dev / regsvr32) so callers fall through.
        // Not noexcept: a (rare) allocation failure propagates to the
        // public TryGetCached* try/catch rather than terminating Explorer.
        std::wstring ResolveCachePath()
        {
            UINT32 len = 0;
            LONG rc = GetCurrentPackageFamilyName(&len, nullptr);
            if (rc != ERROR_INSUFFICIENT_BUFFER || len == 0)
            {
                return {};
            }
            std::wstring pfn(len, L'\0');
            rc = GetCurrentPackageFamilyName(&len, pfn.data());
            if (rc != ERROR_SUCCESS)
            {
                return {};
            }
            // GetCurrentPackageFamilyName writes the trailing NUL inside the
            // buffer; trim it so string concatenation doesn't embed a NUL.
            while (!pfn.empty() && pfn.back() == L'\0')
            {
                pfn.pop_back();
            }
            if (pfn.empty())
            {
                return {};
            }

            wchar_t lad[1024];
            DWORD n = GetEnvironmentVariableW(L"LOCALAPPDATA", lad, 1024);
            if (n == 0 || n >= 1024)
            {
                return {};
            }
            std::wstring path(lad, n);
            path += L"\\Packages\\";
            path += pfn;
            path += L"\\LocalState\\shell-menu-cache.v1.txt";
            return path;
        }

        // Read the UTF-8 cache file and parse `key<TAB>value` lines into
        // g_map (the leading "v1" header line has no tab and is skipped).
        bool LoadFile(std::wstring const& path)
        {
            HANDLE h = CreateFileW(
                path.c_str(), GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
            if (h == INVALID_HANDLE_VALUE)
            {
                return false;
            }
            LARGE_INTEGER size{};
            if (!GetFileSizeEx(h, &size) || size.QuadPart <= 0 || size.QuadPart > kMaxCacheBytes)
            {
                CloseHandle(h);
                return false;
            }
            std::string bytes(static_cast<size_t>(size.QuadPart), '\0');
            DWORD readBytes = 0;
            BOOL ok = ReadFile(h, bytes.data(), static_cast<DWORD>(bytes.size()), &readBytes, nullptr);
            CloseHandle(h);
            if (!ok || readBytes == 0)
            {
                return false;
            }
            bytes.resize(readBytes);

            int wlen = MultiByteToWideChar(CP_UTF8, 0, bytes.data(), static_cast<int>(bytes.size()), nullptr, 0);
            if (wlen <= 0)
            {
                return false;
            }
            std::wstring text(static_cast<size_t>(wlen), L'\0');
            MultiByteToWideChar(CP_UTF8, 0, bytes.data(), static_cast<int>(bytes.size()), text.data(), wlen);

            std::unordered_map<std::wstring, std::wstring> parsed;
            size_t pos = 0;
            while (pos < text.size())
            {
                size_t eol = text.find(L'\n', pos);
                size_t lineLen = (eol == std::wstring::npos) ? (text.size() - pos) : (eol - pos);
                std::wstring line = text.substr(pos, lineLen);
                pos = (eol == std::wstring::npos) ? text.size() : eol + 1;
                if (!line.empty() && line.back() == L'\r')
                {
                    line.pop_back();
                }
                size_t tab = line.find(L'\t');
                if (tab == std::wstring::npos)
                {
                    continue;   // header / malformed line
                }
                parsed.emplace(line.substr(0, tab), line.substr(tab + 1));
            }
            g_map.swap(parsed);
            return true;
        }

        // Re-read the file only when its mtime changed (so Settings / locale
        // changes from the running app reflect on the next right-click).
        // Caller holds g_mu.
        bool EnsureLoaded()
        {
            if (!g_pathResolved)
            {
                g_cachePath = ResolveCachePath();
                g_pathResolved = true;
                g_pathValid = !g_cachePath.empty();
            }
            if (!g_pathValid)
            {
                return false;
            }

            WIN32_FILE_ATTRIBUTE_DATA fad{};
            if (!GetFileAttributesExW(g_cachePath.c_str(), GetFileExInfoStandard, &fad))
            {
                g_haveData = false;
                return false;
            }
            if (g_haveData
                && fad.ftLastWriteTime.dwLowDateTime == g_lastWrite.dwLowDateTime
                && fad.ftLastWriteTime.dwHighDateTime == g_lastWrite.dwHighDateTime)
            {
                return true;
            }
            if (LoadFile(g_cachePath))
            {
                g_lastWrite = fad.ftLastWriteTime;
                g_haveData = true;
                return true;
            }
            g_haveData = false;
            return false;
        }
    }

    bool TryGetCachedString(wchar_t const* key, std::wstring& out) noexcept
    {
        if (key == nullptr || *key == L'\0')
        {
            return false;
        }
        try
        {
            std::lock_guard<std::mutex> lock(g_mu);
            if (!EnsureLoaded())
            {
                return false;
            }
            auto it = g_map.find(key);
            if (it == g_map.end())
            {
                return false;
            }
            out = it->second;
            return true;
        }
        catch (...)
        {
            return false;
        }
    }

    bool TryGetCachedBool(wchar_t const* key, bool& out) noexcept
    {
        std::wstring value;
        if (!TryGetCachedString(key, value))
        {
            return false;
        }
        out = (value == L"1");
        return true;
    }
}
