# OtterZip

> Fast archive tool — Rust core + WinUI 3 (Windows) + SwiftUI (macOS, planned).
> Built for speed. Competes with 7-Zip, WinRAR, Bandizip, PeaZip.

[![Rust CI](https://github.com/otterzip/otterzip/actions/workflows/ci-rust.yml/badge.svg)](./.github/workflows/ci-rust.yml)
[![.NET CI](https://github.com/otterzip/otterzip/actions/workflows/ci-dotnet.yml/badge.svg)](./.github/workflows/ci-dotnet.yml)

---

## 현재 상태 (2026-04-23, Phase 6 Sprint 0)

- ✅ Phase 1 — 용어·스키마·FFI 계약
- ✅ Phase 2 — 컨벤션·구조
- ✅ Phase 3 — 목업 (텍스트 스펙 + HTML)
- ✅ Phase 4 — API 레퍼런스 (Rust / FFI / 셸 확장)
- ✅ Phase 5 — 디자인 토큰 + XAML 스켈레톤
- ✅ **Phase 6 — 병렬 구현 완료** — Sprint 0~6 그린, MSIX 사이드로드 동작
- ✅ Phase 7 — SEO/Security 하드닝 (path component 가드 / 다층 ZIP bomb / OWASP 셀프 점검)
- ✅ Phase 8 — Review (gap 보고서 + 백로그 G1~G7 처리, ABI v5)
- 🚧 Phase 9 — Deployment (사이드로드 / 자기서명 — 정식 채널 미정)

## 아키텍처

```
┌────────────────────────┐
│   WinUI 3 App (C#)     │  Native AOT, MSIX 패키징
│   OtterZip.App          │  다국어/다크/라이트
├────────────────────────┤
│   OtterZip.Interop (C#) │  P/Invoke + SafeHandle
├────────────────────────┤
│   otterzip_ffi.dll (Rust)   │  C ABI (cbindgen 자동 생성)
│   otterzip-ffi          │
├────────────────────────┤
│   otterzip-core (Rust)  │  Archive + 포맷 백엔드
│                        │  libdeflate / zstd / liblzma / ...
└────────────────────────┘

┌────────────────────────┐
│ OtterZip.Shell.dll      │  C++/WinRT · IExplorerCommand
│ (Windows 셸 확장)      │  MSIX uap3:Extension 등록
└────────────────────────┘
```

## 핵심 문서

- [CLAUDE.md](CLAUDE.md) — Claude/개발자 작업 가이드
- [CONVENTIONS.md](CONVENTIONS.md) — 코딩 컨벤션 종합
- [LICENSE.md](LICENSE.md) — 오픈 코어 라이선스 구조
- [docs/01-plan/](docs/01-plan/) — 스키마·성능·구조·라이선스
- [docs/02-design/](docs/02-design/) — 디자인 철학·목업·토큰·시스템
- [docs/03-api/](docs/03-api/) — Rust·FFI·셸 확장 레퍼런스
- [docs/05-build/phase-6-plan.md](docs/05-build/phase-6-plan.md) — 현재 구현 계획

## 개발 환경 셋업

### 요구사항

- **Windows 11** (또는 Windows 10 20H1+)
- **Rust** (`rust-toolchain.toml`이 자동 설치)
- **.NET 9 SDK** (`global.json`이 고정)
- **Visual Studio 2022 17.12+** (Workloads: .NET desktop, C++ desktop, Windows App SDK)
- **Windows App SDK 1.6+**

### 빌드

```powershell
# 1. Rust (네이티브 otterzip_ffi.dll)
cargo build --workspace

# 2. .NET (빌드 타깃이 Rust 산출물 자동 복사)
dotnet build OtterZip.sln

# 3. (개발자) CLI로 smoke
cargo run -p otterzip-cli

# 4. (개발자) 벤치
cargo bench -p otterzip-core
```

### 실행

Visual Studio에서 `OtterZip.App`을 스타트업 프로젝트로 설정 후 F5.

## 라이선스

- `crates/**` — **MIT OR Apache-2.0** (Rust 생태계 듀얼)
- `app/**` — **Proprietary** (All Rights Reserved)
- 자세히: [LICENSE.md](LICENSE.md)

## 제3자 고지

- [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)
- 특히 **unrar** (© Alexander Roshal) — RAR 해제 전용

## 기여

현재는 코어 팀 비공개 기여. OSS 기여 오픈은 v0.2.0 이후 예정.

---

**성능 규약:** 세상에서 가장 빠르지는 않다. 하지만 **경쟁 아카이브 GUI 도구보다는 빠르다.** 측정: Silesia corpus 기준. ([docs/01-plan/performance.md](docs/01-plan/performance.md))
