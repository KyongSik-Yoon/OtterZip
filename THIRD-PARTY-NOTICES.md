# SpanZIP — Third-Party Notices

> 이 문서는 SpanZIP 배포 바이너리에 포함되는 제3자 라이브러리의 라이선스 고지를 기록합니다.
> 법적 요구사항 충족을 위한 필수 문서입니다. **릴리스 전 자동 생성·검증**해야 합니다.

## 생성 방법

```
cargo install cargo-about
cargo about generate about.hbs > THIRD-PARTY-NOTICES.md
```

CI에서 차이 발생 시 PR 블록. 수동 편집 금지 (자동 생성 구간 외).

---

## Rust 코어 의존성 (예상)

아래 표는 `Cargo.toml` 확정 후 `cargo about`으로 자동 갱신됩니다.

| Crate | 버전 | 라이선스 | 용도 |
|---|---|---|---|
| `libdeflater` | 1.x | MIT | DEFLATE 고속 백엔드 |
| `libdeflate-sys` | — | MIT | libdeflate C 바인딩 |
| `zstd` | 0.13.x | MIT OR Apache-2.0 | Zstandard 압축 |
| `zstd-sys` | — | MIT OR Apache-2.0 | zstd C 바인딩 |
| `bzip2` | 0.5.x | MIT OR Apache-2.0 | bzip2 압축 |
| `bzip2-sys` | — | MIT OR Apache-2.0 | libbz2 바인딩 |
| `xz2` | 0.1.x | MIT | LZMA/xz 압축 (liblzma) |
| `lzma-sys` | — | MIT | liblzma 바인딩 |
| `flate2` | 1.x | MIT OR Apache-2.0 | zlib-ng 백엔드 |
| `tar` | 0.4.x | MIT OR Apache-2.0 | TAR 파싱 |
| `zip` | 2.x | MIT | ZIP 파싱/작성 |
| `sevenz-rust` | 0.6.x | Apache-2.0 | 7z 파싱 |
| `unrar` | 0.5.x | **RAR License + MIT** | RAR 추출 (유일 합법 경로) |
| `rayon` | 1.x | MIT OR Apache-2.0 | 데이터 병렬 |
| `memmap2` | 0.9.x | MIT OR Apache-2.0 | 메모리 매핑 |
| `thiserror` | 2.x | MIT OR Apache-2.0 | 에러 derive |
| `tracing` | 0.1.x | MIT | 로깅/트레이싱 |
| `zeroize` | 1.x | MIT OR Apache-2.0 | 비밀 메모리 제로화 |

## 특별 주의: unrar / RAR License

`unrar` 크레이트는 WinRAR의 `unrar.dll` / `unrar` 소스를 래핑합니다. WinRAR의 라이선스 조건은 다음 사항을 요구합니다:

1. **RAR 해제 전용.** RAR 아카이브 생성에 사용 금지 — SpanZIP은 이를 준수 (`ErrorCode::FeatureDisabled`).
2. **리버스 엔지니어링을 통한 RAR 포맷 구현 금지.** SpanZIP은 공식 unrar 라이브러리만 사용, 독자 구현 없음.
3. **배포 시 WinRAR 저작권 고지 포함 필수.** 본 문서와 애플리케이션 정보 화면에 표시.

**필수 고지문 (About 화면에 표시 예정):**
```
이 제품은 RAR 아카이브 해제를 위해 Alexander Roshal의 unrar 소스를
사용합니다. Copyright (c) Alexander Roshal. 해당 소스는 RAR 해제
목적에 한해 사용이 허가되었으며, 이를 RAR 아카이브 생성에 사용할 수
없습니다. https://www.rarlab.com/
```

## .NET / WinUI 3 런타임 구성요소

| 구성요소 | 라이선스 | 비고 |
|---|---|---|
| .NET 9 Runtime | MIT | Native AOT 번들 |
| Microsoft.WindowsAppSDK (WinUI 3) | MIT | 앱에 포함 |
| CommunityToolkit.Mvvm | MIT | MVVM 소스 제너레이터 |
| `System.Text.Json` 등 BCL | MIT | .NET 표준 |

MIT 라이선스 텍스트 전문은 `dotnet list package --include-transitive`로 확인 후 배포 아티팩트에 `NOTICE.txt`로 동봉합니다.

---

## About 화면 템플릿 (앱 UI에 반영)

```
SpanZIP {version}
Copyright © 2026 SpanZIP. All Rights Reserved.

Built with:
  • Rust core (MIT OR Apache-2.0) — see /legal/rust-core
  • libdeflate (MIT) — high-speed DEFLATE
  • zstd (BSD-3) — Yann Collet / Meta
  • liblzma / xz-utils (public domain)
  • WinUI 3 / .NET 9 (MIT) — Microsoft

This product uses the unrar source from Alexander Roshal for RAR
decompression. RAR archive creation is not supported.

Full third-party licenses: Help → About → View Licenses
```

---

## CI 게이트

릴리스 빌드 파이프라인에 다음 체크 포함:

1. `cargo deny check licenses` — 허용 라이선스 화이트리스트 검증
2. `cargo about generate` — 본 문서 자동 갱신, diff 있으면 실패
3. `dotnet list package` — C# 의존성 스냅샷 기록
4. About 화면 고지문 존재 여부 UI 테스트

### `deny.toml` (`cargo-deny`) 예시

```toml
[licenses]
allow = [
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-DFS-2016",
    "CC0-1.0",
]
# GPL / LGPL / AGPL / MPL-2.0 검토 필요 — 기본 거부
deny = ["GPL-2.0", "GPL-3.0", "AGPL-3.0"]
# unrar는 별도 예외 처리
exceptions = [
    { allow = ["RARv5"], name = "unrar" },
]
```
