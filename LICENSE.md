# OtterZip 라이선스 개요

OtterZip 저장소는 **두 가지 라이선스 영역**으로 구분됩니다.

## 1. Rust 코어 (`crates/**`) — 오픈 소스

`crates/otterzip-core`, `crates/otterzip-ffi`, `crates/otterzip-bench`, `crates/otterzip-cli`의 모든 소스 파일은 다음 라이선스 중 **하나를 선택**하여 사용할 수 있습니다:

- **MIT License** — 전문: [`crates/LICENSE-MIT`](crates/LICENSE-MIT)
- **Apache License, Version 2.0** — 전문: [`crates/LICENSE-APACHE`](crates/LICENSE-APACHE)

SPDX 식별자: `MIT OR Apache-2.0`

이는 Rust 생태계의 표준 듀얼 라이선스 관행입니다. 사용자는 두 라이선스 중 프로젝트에 적합한 것을 선택하여 적용할 수 있습니다.

### 기여 (Contributions)

기여자가 OtterZip의 Rust 코어에 의도적으로 제출하는 모든 기여물은 추가 조건 없이 Apache-2.0 라이선스에 따라 듀얼 라이선스됨에 동의한 것으로 간주합니다 (Apache-2.0 §5).

## 2. 애플리케이션 (`app/**`) — Proprietary

`app/OtterZip.App`, `app/OtterZip.Interop`, `app/OtterZip.App.Tests`의 모든 소스 파일, XAML, 에셋, 아이콘, 브랜드는 **All Rights Reserved** 입니다.

- 전문: [`app/LICENSE`](app/LICENSE)
- 무단 복제·수정·재배포·역공학을 금합니다.
- 상업 사용 시 별도 라이선스 계약이 필요합니다.

## 3. 제3자 구성요소

배포 바이너리에 포함되는 제3자 라이브러리의 라이선스 고지는 다음 문서를 참조하십시오:

- [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md)

## 4. 문서 · 벤치 · 스크립트

- `docs/**`, `bench/scripts/**`, `scripts/**` — 별도 명시 없는 한 Rust 코어와 동일 (`MIT OR Apache-2.0`)
- `bench/corpus/**` — 각 코퍼스 원저작자 라이선스를 따름 (외부 리소스)

## 5. 브랜드 · 상표

"OtterZip" 이름, 로고, 아이콘은 라이선스 범위 **밖**이며 소유자에게 권리가 유보됩니다.
Rust 코어를 fork하여 파생 제품을 만드는 경우 "OtterZip" 명칭 사용은 허가되지 않습니다.

## 6. 요약표

| 경로 | 라이선스 |
|---|---|
| `crates/**` | `MIT OR Apache-2.0` |
| `app/**` | Proprietary (All Rights Reserved) |
| `docs/**` | `MIT OR Apache-2.0` |
| `bench/scripts/**`, `scripts/**` | `MIT OR Apache-2.0` |
| `bench/corpus/**` | 원저작자 라이선스 |
| "OtterZip" 상표 | All Rights Reserved |
