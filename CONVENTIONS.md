# SpanZIP 코딩 컨벤션

> 본 문서는 Rust 코어 / FFI 경계 / C# WinUI3 / 공통 워크플로의 규칙을 통합 정의합니다.
> [CLAUDE.md](CLAUDE.md)의 핵심 규칙과 함께 프로젝트 헌법 역할.

- 구조·레이아웃: [docs/01-plan/structure.md](docs/01-plan/structure.md)
- 용어: [docs/01-plan/glossary.md](docs/01-plan/glossary.md)
- FFI 계약: [docs/01-plan/schema.md](docs/01-plan/schema.md)
- 성능 규약: [docs/01-plan/performance.md](docs/01-plan/performance.md)

---

## 0. 불변 원칙

1. **성능 > 아름다움.** 모든 컨벤션은 성능 미션과 충돌 시 성능이 이긴다.
2. **측정 없는 최적화 금지.** "빠를 것이다"가 아니라 "빠르다(벤치 결과)"로 말한다.
3. **FFI 경계는 계약이다.** schema.md의 시그니처 변경 시 ABI 버전 증가 필수.
4. **용어는 glossary.md의 영문 식별자**만 사용.

---

## 1. Rust 코어 컨벤션

### 1.1 도구체인

- **Rust 버전:** `stable` 최신 (`rust-toolchain.toml`로 고정)
- **Edition:** `2021`
- **MSRV:** 선언하지 않음 (툴링 전용 프로젝트, 라이브러리 배포 아님)
- **포맷터:** `rustfmt` 프로젝트 루트에 `rustfmt.toml`:

```toml
# rustfmt.toml
edition = "2021"
max_width = 100
use_field_init_shorthand = true
use_try_shorthand = true
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
newline_style = "Unix"
```

- **린터:** `clippy` `pedantic` + `nursery` + 성능 렌트 강제:

```toml
# clippy.toml
avoid-breaking-exported-api = false
cognitive-complexity-threshold = 30
too-many-arguments-threshold = 10
```

```rust
// 각 crate의 lib.rs 상단
#![warn(
    clippy::pedantic,
    clippy::nursery,
    clippy::inefficient_to_string,
    clippy::large_stack_arrays,
    clippy::large_types_passed_by_value,
    clippy::mut_mut,
    clippy::needless_pass_by_value,
    clippy::redundant_allocation,
    clippy::unnecessary_box_returns,
    clippy::useless_conversion,
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,  // thiserror로 충분
)]
```

- **CI:** `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo bench` 전부 블로킹.

### 1.2 네이밍

| 대상 | 규칙 | 예 |
|---|---|---|
| 크레이트 | kebab-case | `spanzip-core`, `spanzip-ffi` |
| 모듈 | snake_case | `mod archive_backend` |
| 타입/trait | UpperCamelCase | `Archive`, `ArchiveBackend` |
| 함수/메서드/변수 | snake_case | `extract_all`, `entry_count` |
| 상수 | SCREAMING_SNAKE | `MAX_VOLUME_COUNT` |
| 제네릭 파라미터 | 단일 대문자 또는 명시적 이름 | `T`, `R: Read` |
| 약어 | 두문자어도 camel case | `ZipCrypto` (O), `ZIPCrypto` (X) |

### 1.3 에러 처리

- **라이브러리 크레이트(`spanzip-core`, 포맷 백엔드 크레이트):** `thiserror` 필수, `anyhow` **금지**
- **바이너리/통합 테스트:** `anyhow` 허용
- **패닉 금지 영역:** `spanzip-core`, `spanzip-ffi`의 모든 공개 함수
  - `unwrap()`, `expect()`, `panic!`, `unreachable!()` 사용 시 **SAFETY 주석 또는 논리적 근거 주석 필수**
  - `Vec::get(i).unwrap()` 대신 `?` 또는 명시적 에러 리턴
- **FFI 반환 시 오류 매핑:** 내부 `SpanzipError` → `ErrorCode` (i32) + TLS 메시지 저장

```rust
// 예: spanzip-core/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum SpanzipError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported format: {detected:?}")]
    UnsupportedFormat { detected: Option<String> },
    #[error("corrupted archive: {reason}")]
    Corrupted { reason: String },
    #[error("wrong password")]
    WrongPassword,
    #[error("missing volume {index}")]
    MissingVolume { index: u32 },
    #[error("operation canceled")]
    Canceled,
    #[error("feature disabled: {feature}")]
    FeatureDisabled { feature: &'static str },
}
```

### 1.4 `unsafe` 규칙

- **모든 `unsafe` 블록에 `// SAFETY: ...` 주석 필수** (clippy `undocumented_unsafe_blocks` 활성)
- SAFETY 주석은 다음을 설명:
  1. **어떤 불변조건(invariant)이 성립하는지**
  2. **왜 그 불변조건이 이 시점에 성립하는지**
  3. **해당 불변조건이 깨지면 어떤 결과인지**
- `unsafe fn`을 선언하는 경우 **문서 주석에 `# Safety` 섹션** 필수

```rust
// ✅ 좋음
// SAFETY: `ptr`은 `spanzip_archive_open`이 반환한 유효 포인터이며,
// `spanzip_archive_close` 호출 전까지 살아있음이 호출자 계약(schema.md §4).
// 동시 다른 스레드에서 접근 없음 (§4 스레드 모델).
let archive = unsafe { &mut *ptr };

// ❌ 나쁨 (주석 없음)
let archive = unsafe { &mut *ptr };
```

### 1.5 성능 패턴 (hot path)

- **금지:** hot loop 내 `String::from`, `format!`, `to_string()` — 대신 `&str`/`write!`로
- **금지:** hot loop 내 `Box<dyn Trait>` 호출 — 제네릭으로 모노모픽
- **금지:** hot loop 내 할당 — `Vec::with_capacity` 또는 스크래치 버퍼 재사용
- **권장:** `#[inline]`은 "FFI 경계 래퍼"와 "hot loop 내 불가피한 간접 호출"에만. 남발 금지 (컴파일 시간 폭증)
- **권장:** `#[repr(C)]` FFI 구조체, `#[repr(transparent)]` 뉴타입
- **권장:** CPU 바운드 병렬은 `rayon`. async runtime(tokio) **도입 금지**
- **측정:** `criterion` 벤치마크를 모든 공개 해제 경로마다 작성. 회귀 -5% 시 PR 블록

### 1.6 허용/금지 크레이트

**허용 (기본):**
- `thiserror`, `tracing`, `rayon`, `memmap2`, `zeroize`
- 백엔드: `libdeflater`, `zstd`, `bzip2`, `xz2`, `tar`, `zip`, `sevenz-rust`, `unrar`

**금지:**
- `anyhow` (라이브러리 크레이트에서)
- `tokio`, `async-std`, `futures-executor` (async runtime 전반)
- `lazy_static` → `std::sync::OnceLock` 또는 `once_cell`
- `serde_json` in hot path → 필요 시 배치 직렬화만

**추가는 PR 본문에 근거 명시.**

---

## 2. FFI 경계 컨벤션

### 2.1 네이밍

- **함수:** `spanzip_<namespace>_<verb>` — 예: `spanzip_archive_open`, `spanzip_iterator_next`
- **타입:** `Spanzip<CamelCase>` — 예: `SpanzipArchive`, `SpanzipEntryView`
- **열거형 값:** `SPANZIP_<NAMESPACE>_<NAME>` — 예: `SPANZIP_FORMAT_ZIP`, `SPANZIP_ERROR_WRONG_PASSWORD`
- **콜백 typedef:** `Spanzip<Noun>Cb` — 예: `SpanzipProgressCb`

### 2.2 시그니처 규칙

- **문자열:** 항상 `const char* utf8, size_t len` 쌍. null-terminated에 의존 **금지**
- **출력 핸들:** 마지막 인자 `SpanzipXxx** out_handle`. 호출자가 nullable 체크 후 소유
- **배열:** `T* buf, size_t len` 쌍 + 필요 시 `size_t* out_written`
- **반환값:** 모든 fallible 함수는 `int32_t` 오류 코드. 0 = OK, 음수 = 오류
- **불변 포인터:** `const SpanzipArchive*`로 읽기 전용 명시
- **생명주기:** `_new`/`_free` 또는 `_open`/`_close` 쌍을 항상 함께 제공, 헤더 주석에 소유권 명시

### 2.3 cbindgen 설정

`spanzip-ffi/cbindgen.toml`:

```toml
language = "C"
header = "/* SpanZIP C API — Auto-generated by cbindgen. DO NOT EDIT. */"
include_guard = "SPANZIP_H"
cpp_compat = true
pragma_once = true
style = "type"
documentation = true
usize_is_size_t = true

[parse]
parse_deps = false

[export]
prefix = "Spanzip"  # 타입 접두어
item_types = ["enums", "structs", "opaque", "functions"]

[export.rename]
# Rust 식별자 → C 식별자 매핑 (필요 시)

[enum]
prefix_with_name = true
rename_variants = "ScreamingSnakeCase"
```

- C 헤더는 `spanzip-ffi/include/spanzip.h`로 출력, **빌드 아티팩트**로 취급 (Git 체크인하되 수정 금지)
- ABI 호환성 테스트: CI에서 `spanzip_abi_version()` 증가 없이 헤더 diff 발생하면 실패

### 2.4 핸들 불변조건

- **opaque 핸들은 Rust `Box<T>`로 생성, `Box::into_raw`로 전환, `Box::from_raw`로 해제**
- **사용자 정의 RAII 없음.** Rust 쪽 `Drop` 구현에 의존
- **모든 FFI 함수는 null 체크 + invalid handle 감지** 후 `ErrorCode::InvalidHandle` 반환

```rust
// 패턴
#[no_mangle]
pub extern "C" fn spanzip_archive_close(handle: *mut SpanzipArchive) {
    if handle.is_null() { return; }
    // SAFETY: 호출자 계약(§2.4)에 따라 `handle`은 `spanzip_archive_open`이 반환한 값.
    // 본 호출 이후 해당 포인터의 사용은 UB (호출자 책임).
    unsafe { drop(Box::from_raw(handle)); }
}
```

### 2.5 FFI 호출에서의 panic 방지

- **모든 `#[no_mangle] extern "C" fn`은 `std::panic::catch_unwind`로 감싸기**
- panic이 FFI 경계를 넘으면 UB — 반드시 잡아서 `ErrorCode::Generic`으로 변환

```rust
#[no_mangle]
pub extern "C" fn spanzip_archive_open(...) -> i32 {
    catch_unwind_to_error(|| -> Result<(), SpanzipError> {
        // 실제 로직
    })
}
```

---

## 3. C# / WinUI 3 컨벤션

### 3.1 도구체인

- **.NET:** 9.0 LTS
- **언어 버전:** C# 13
- **배포 형태:** Native AOT (`<PublishAot>true</PublishAot>`)
- **포맷터:** `.editorconfig` + `dotnet format` CI
- **분석기:** `Microsoft.CodeAnalysis.NetAnalyzers`, `StyleCop.Analyzers`, `Meziantou.Analyzer`
- **Nullable:** `<Nullable>enable</Nullable>` 전역
- **ImplicitUsings:** `disable` (명시적 import 선호 — AOT 디버깅 용이)

### 3.2 네이밍

| 대상 | 규칙 | 예 |
|---|---|---|
| 네임스페이스 | PascalCase | `SpanZIP.App.Views` |
| 클래스/인터페이스/레코드 | PascalCase | `ArchiveViewModel`, `IArchiveService` |
| 메서드 | PascalCase | `OpenArchiveAsync` |
| 프로퍼티 | PascalCase | `IsExtracting` |
| 필드 private | `_camelCase` | `_archiveService` |
| 지역 변수/파라미터 | camelCase | `archivePath` |
| 상수 | PascalCase | `MaxRecentFiles` |
| 인터페이스 | `I` 접두어 | `IArchiveService` |

### 3.3 Native AOT 제약

- **금지:**
  - `System.Reflection.Emit` 전반
  - 동적 어셈블리 로딩 (`Assembly.LoadFrom`)
  - `JsonSerializer` 리플렉션 기반 — 대신 `JsonSerializerContext` source generator
  - `Expression<T>` 컴파일 (LINQ to objects는 OK)
  - `Type.GetType(string)` 동적 — AOT 트리밍 경고
- **필수:**
  - 모든 P/Invoke는 `[LibraryImport]` (source generator) — `[DllImport]` 금지
  - JSON 직렬화는 `[JsonSerializable]` 파셜 컨텍스트 사용
  - 경고 `IL2xxx` / `IL3xxx` 발생 시 PR 블록

### 3.4 P/Invoke 래퍼 패턴

**얇은 바인딩 레이어 + 안전한 공개 래퍼** 이중 구조:

```
SpanZIP.Interop/
├── Native/
│   └── NativeMethods.cs     // [LibraryImport] 파셜 클래스, internal static
├── SafeHandles/
│   └── ArchiveHandle.cs     // SafeHandle 파생, Close() = spanzip_archive_close
└── Archive.cs                // 공개 API, 예외 throw, IDisposable
```

```csharp
// NativeMethods.cs — 원시 P/Invoke (internal only)
internal static partial class NativeMethods
{
    private const string Lib = "spanzip";

    [LibraryImport(Lib, EntryPoint = "spanzip_archive_open", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial int ArchiveOpen(
        string pathUtf8, nuint pathLen,
        uint mode,
        string? passwordUtf8, nuint passwordLen,
        out IntPtr outHandle);

    [LibraryImport(Lib, EntryPoint = "spanzip_archive_close")]
    internal static partial void ArchiveClose(IntPtr handle);

    [LibraryImport(Lib, EntryPoint = "spanzip_last_error_message", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial string? LastErrorMessage();
}

// ArchiveHandle.cs — SafeHandle로 소유권 관리
public sealed class ArchiveHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    public ArchiveHandle() : base(ownsHandle: true) { }
    protected override bool ReleaseHandle()
    {
        NativeMethods.ArchiveClose(handle);
        return true;
    }
}

// Archive.cs — 공개 API (예외·async wrap)
public sealed class Archive : IDisposable
{
    private readonly ArchiveHandle _handle;
    public static Archive Open(string path, ArchiveOpenMode mode, string? password = null)
    {
        int rc = NativeMethods.ArchiveOpen(
            path, (nuint)Encoding.UTF8.GetByteCount(path),
            (uint)mode,
            password, password is null ? 0 : (nuint)Encoding.UTF8.GetByteCount(password),
            out IntPtr raw);
        SpanzipException.ThrowIfError(rc);
        var handle = new ArchiveHandle();
        Marshal.InitHandle(handle, raw); // .NET 6+
        return new Archive(handle);
    }
    public void Dispose() => _handle.Dispose();
}
```

### 3.5 UI 스레드 규칙

- **모든 아카이브 작업은 ThreadPool/Task.Run에서** — UI 스레드 블로킹 절대 금지
- **진행률 콜백은 네이티브 호출 스레드에서 실행** → `DispatcherQueue.TryEnqueue`로 UI 스레드 마샬링
- **취소:** .NET `CancellationToken` → 진행률 콜백이 네이티브 측에 취소 신호 반환

### 3.6 XAML / 데이터 바인딩

- **`{x:Bind}` 우선**, `{Binding}`은 동적 DataContext가 필요한 경우만 (AOT 친화)
- 컴파일된 바인딩 경로 위반 시 빌드 에러로 승격
- ViewModel: MVVM Toolkit (`CommunityToolkit.Mvvm`) — AOT 지원
- `[ObservableProperty]`, `[RelayCommand]` source generator 사용

### 3.7 async 규칙

- **모든 async 메서드는 `Async` 접미어**
- **ConfigureAwait(false)**: 라이브러리 레이어(Interop/ViewModel 외) 필수
- **`async void` 금지** (이벤트 핸들러 예외)
- **장시간 작업은 Task.Run + CancellationToken** 패턴

### 3.8 다국어 (i18n) — **하드 룰**

> **모든 사용자 노출 문자열의 하드코딩은 절대 금지한다.** 위반 시 PR 블록, 예외 사유 불허.

#### 필수 조치

- **XAML:** 모든 사용자 노출 속성은 `x:Uid` 기반으로 `.resw`에 바인딩
  - 대상 속성: `Text`, `Content`, `Header`, `PlaceholderText`, `ToolTipService.ToolTip`, `AutomationProperties.Name`, `AutomationProperties.HelpText`
- **C#:** 런타임 문자열은 `ResourceLoader.GetForViewIndependentUse().GetString("Key")` 또는 ViewModel 프로퍼티 + 리소스 참조
- **포맷 문자열:** `CompositeFormat` (.NET 8+, AOT 친화) 사용. `string.Format`는 포맷 문자열 자체가 리소스에서 올 때만.
- **문화권 의존 포맷:** 파일 크기·날짜·숫자는 `CultureInfo.CurrentUICulture` 기반 포맷터 (절대 하드코딩된 포맷 문자열 사용 금지)

#### 파일 구조

```
SpanZIP.App/
└── Strings/
    ├── ko-KR/
    │   └── Resources.resw        # 기본 언어 (primary)
    ├── en-US/
    │   └── Resources.resw        # 국제판
    └── (향후) ja-JP, zh-Hans, ...
```

- `ko-KR` 기본 + `en-US` 1차 확장. 추가 언어는 `.resw`만 늘림
- **누락 키 검사 CI:** 모든 언어 폴더가 동일 키 집합 보유해야 함. 누락 시 빌드 실패

#### 금지 패턴 (PR 자동 거부)

```xml
<!-- ❌ 금지 -->
<Button Content="추출" />
<TextBlock Text="파일을 선택하세요" />

<!-- ✅ 필수 -->
<Button x:Uid="ExtractButton" />
<TextBlock x:Uid="SelectFilePrompt" />
```

```csharp
// ❌ 금지
statusLabel.Text = "완료되었습니다";
Dialog.ShowAsync("오류", "파일을 열 수 없습니다");

// ✅ 필수
statusLabel.Text = _resources.GetString("CompletionMessage");
await ShowDialogAsync(
    _resources.GetString("ErrorDialogTitle"),
    _resources.GetString("FileOpenFailedMessage"));
```

#### 예외 (하드코딩 허용되는 것)

- 디버그·진단 로그 (`tracing`, `ILogger.LogDebug`) — 사용자에게 노출 안 됨
- 내부 프로토콜 식별자 (예: FFI 에러 코드 이름)
- 포맷 자체의 이름 표시 (`"ZIP"`, `"7z"` — 제품명은 번역하지 않음)

#### .resw 키 네이밍 컨벤션 (확정 2026-04-30)

```
<Screen>_<Element>.<Attribute>
```

- **Screen / Element 사이 구분자: `_` (밑줄)** — 점(`.`) 사용 금지
- **Attribute 앞 구분자: `.` (점)** — `x:Uid` 가 자동 매핑하는 표준 형식
- 예시:
  - ✅ `Settings_DialogTitle.Title`
  - ✅ `ConfigPanel_FormatLabel.Text`
  - ✅ `ExtractDialog_PrimaryButton.Text`
  - ❌ `Settings.DialogTitle.Title` (점만)
  - ❌ `ConfigPanelFormatLabelText` (구분자 없음)
- 문서(예: mockup-spec.md)에서 키를 인용할 때도 동일 표기. 설계와 구현의 grep으로 일치 확인 가능해야 함.

#### CI 검사

- **XAML 하드코딩 탐지:** `grep -E 'Text="[^{][^"]+[가-힣a-zA-Z]'` 스크립트로 리터럴 감지 시 실패
- **.resw 키 동기화:** 모든 언어 폴더 키 비교 스크립트
- **.resw 키 네이밍:** `^[A-Z][A-Za-z0-9]+_[A-Z][A-Za-z0-9]+\.[A-Z][A-Za-z]+$` 정규식 위반 시 PR 블록 (Phase 6+ 추가 예정)

### 3.9 테마 (다크/라이트) — **하드 룰**

> **모든 색상·브러시 값의 하드코딩은 절대 금지한다.** 위반 시 PR 블록.

#### 필수 조치

- **XAML:** `Background`, `Foreground`, `BorderBrush`, 모든 `Color` 속성은 반드시
  - `{ThemeResource ...}` (시스템 제공 또는 프로젝트 정의 테마 리소스)
  - 또는 프로젝트 테마 딕셔너리에서 `Dark`/`Light`/`HighContrast` 모두 정의된 키
- **C#:** `new SolidColorBrush(Colors.X)` 절대 금지
  - `Application.Current.Resources["BrushKeyName"]` 또는 `FrameworkElement.Resources` Lookup만

#### 테마 리소스 구조

```
SpanZIP.App/
└── Themes/
    ├── Colors.xaml           # 기본 ThemeDictionary
    ├── Brushes.xaml          # 재사용 브러시
    └── Generic.xaml          # 테마 머지
```

```xml
<!-- Colors.xaml -->
<ResourceDictionary xmlns="...">
    <ResourceDictionary.ThemeDictionaries>
        <ResourceDictionary x:Key="Light">
            <Color x:Key="ArchiveListRowAltColor">#F5F5F5</Color>
        </ResourceDictionary>
        <ResourceDictionary x:Key="Dark">
            <Color x:Key="ArchiveListRowAltColor">#2B2B2B</Color>
        </ResourceDictionary>
        <ResourceDictionary x:Key="HighContrast">
            <Color x:Key="ArchiveListRowAltColor">{ThemeResource SystemColorWindowColor}</Color>
        </ResourceDictionary>
    </ResourceDictionary.ThemeDictionaries>
</ResourceDictionary>
```

#### 금지 패턴

```xml
<!-- ❌ 금지 -->
<Border Background="White" />
<TextBlock Foreground="#FF000000" />
<Grid Background="{StaticResource SomeColor}" />  <!-- 색상은 Static 금지 -->

<!-- ✅ 필수 -->
<Border Background="{ThemeResource LayerFillColorDefaultBrush}" />
<TextBlock Foreground="{ThemeResource TextFillColorPrimaryBrush}" />
<Grid Background="{ThemeResource ArchiveListRowAltBrush}" />
```

```csharp
// ❌ 금지
border.Background = new SolidColorBrush(Colors.White);
var black = Color.FromArgb(255, 0, 0, 0);

// ✅ 필수
border.Background = (Brush)Application.Current.Resources["LayerFillColorDefaultBrush"];
```

#### HighContrast 대응

- 접근성 요구 — **3개 테마 모두**(`Light` / `Dark` / `HighContrast`) 동일 키 제공
- `HighContrast`에서는 원칙적으로 `{ThemeResource SystemColor*}`에 위임 (Windows 시스템 색 사용)
- 커스텀 색이 꼭 필요한 경우에만 별도 리소스

#### 테마 전환 검증

- 개발 시 WinUI 테마 전환 도구로 Light ↔ Dark 즉시 전환 테스트
- UI 테스트에 Theme override 기반 스크린샷 비교 항목 포함

#### CI 검사

- **리터럴 색상 탐지:** `grep -E '(Background|Foreground|BorderBrush|Color|Fill)="(#|[A-Z][a-z])'` (Theme/Static 참조 제외 후)
- **`new SolidColorBrush\(` grep:** C# 코드 전체 스캔, Interop 외 모든 디렉터리 차단

---

## 4. 프로젝트 구조

별도 문서: [docs/01-plan/structure.md](docs/01-plan/structure.md)

---

## 5. 파일·문서 컨벤션

### 5.1 파일 이름

- **Rust:** `snake_case.rs`. 모듈명과 일치
- **C#:** `PascalCase.cs`. 파일당 하나의 public 타입 원칙
- **XAML:** `PascalCase.xaml` + `.xaml.cs` code-behind (code-behind 최소화)
- **문서:** `kebab-case.md`. 단 `README.md`, `CLAUDE.md`, `CONVENTIONS.md`, `CHANGELOG.md`는 대문자
- **설정:** 도구 관례 (`rustfmt.toml`, `.editorconfig`, `cbindgen.toml`)

### 5.2 라이선스 헤더

- **파일 헤더는 넣지 않는다.** 저장소 루트 `LICENSE`로 충분. 헤더는 diff 노이즈만 증가.
- 예외: 외부 코드 포팅 시 원본 라이선스 고지 의무가 있으면 해당 파일 상단에 **최소한의 출처 + SPDX 라인**만.

### 5.3 주석 규칙

- **WHY 주석만.** WHAT은 코드와 식별자로 충분.
- 공개 API는 doc 주석 필수 (Rust `///`, C# `///`)
- TODO/FIXME는 이슈 번호 병기: `// TODO(#42): ...`

---

## 6. Git 워크플로

### 6.1 브랜치 전략

- **트렁크 기반 + 짧은 feature 브랜치**
- `main` = 항상 배포 가능
- `feat/*`, `fix/*`, `perf/*`, `refactor/*`, `docs/*` 브랜치
- PR 머지는 **squash** 기본. 히스토리 선형 유지

### 6.2 커밋 메시지 — Conventional Commits

```
<type>(<scope>): <subject>

<body (optional)>

<footer (issues, breaking changes)>
```

**type:** `feat` | `fix` | `perf` | `refactor` | `docs` | `test` | `chore` | `build` | `ci`

**scope:** `core` | `ffi` | `winui` | `zip` | `7z` | `rar` | `tar` | `build` | `bench`

**예:**
```
perf(zip): switch DEFLATE backend to libdeflate (2.3× faster)

Silesia corpus 압축 해제 평균 287 MB/s → 662 MB/s (Intel i7-12700K).
libdeflater 1.20 크레이트 도입, flate2 의존 제거.

Closes #57
```

### 6.3 PR 체크리스트

- [ ] `cargo fmt --check` / `dotnet format --verify-no-changes` 통과
- [ ] `cargo clippy -- -D warnings` / C# 분석기 경고 0
- [ ] 모든 테스트 통과
- [ ] FFI 시그니처 변경 시 `spanzip_abi_version` 증가 + `docs/CHANGELOG-ffi.md` 갱신
- [ ] 성능 관련 변경 시 벤치 결과 첨부 (Silesia 코퍼스 최소)
- [ ] 새 용어 도입 시 glossary.md 업데이트
- [ ] `unsafe` 블록 추가 시 SAFETY 주석 존재

### 6.4 PR 크기

- **500 LOC 이하 권장.** 초과 시 분할.
- 기능 추가 + 리팩터 섞지 않기.

---

## 7. 환경 변수·설정

일반 웹 프로젝트와 달리 이 프로젝트는 **런타임 환경 변수 의존이 거의 없음**. 주 설정은:

| 항목 | 위치 | 비고 |
|---|---|---|
| 벤치마크 코퍼스 경로 | `SPANZIP_BENCH_CORPUS` env | CI·로컬 벤치용 |
| 로그 레벨 | `SPANZIP_LOG=debug` | `tracing` 필터 |
| 개발자 크래시 덤프 | `SPANZIP_DUMP_DIR` | 자동 경로 기본값 |

- **사용자 설정은 OS 표준 위치**에 저장 (Windows: `%LOCALAPPDATA%\SpanZIP\`, macOS: `~/Library/Application Support/SpanZIP/`)
- **비밀 키·API 키 없음** (오프라인 데스크톱 도구)
- `.env*` 파일 구조 미채택

---

## 8. 아키텍처 레이어링

### 8.1 Rust 코어 내부

```
spanzip-core/
├── archive/       # Archive, OpenMode, 공용 트레잇
├── backends/      # 포맷별 구현
│   ├── zip/
│   ├── sevenz/
│   ├── rar/
│   ├── tar/
│   └── gzip/
├── io/            # memmap, span reader, progress
├── parallel/      # rayon 유틸, 작업 분할
└── error.rs
```

**의존 방향:** `backends` → `archive` → `io` → `error`. 역방향 금지.

### 8.2 C# 애플리케이션

```
SpanZIP.App/
├── Views/            # XAML (presentation)
├── ViewModels/       # MVVM Toolkit 기반
├── Services/         # 비즈니스 오케스트레이션
└── Interop/          # P/Invoke + SafeHandle (infrastructure)

SpanZIP.Interop/      # 별도 어셈블리 권장 (UI 없는 콘솔 테스트용)
```

**의존 방향:** `Views` → `ViewModels` → `Services` → `Interop`. 역방향 금지. `Views`에서 `Interop` 직접 import 금지.

### 8.3 크로스 레이어 규칙

- Rust 코어는 C# 존재를 모름 (Rust는 자신이 UI 있는지 모름)
- C# 쪽에서만 "UI 스레드" 개념 존재
- FFI 경계가 유일한 통신 채널

---

## 9. 재사용·확장성

- **Strategy 패턴** — 포맷별 백엔드는 `ArchiveBackend` 트레잇 구현. 새 포맷 추가 = 트레잇 구현 추가 + 팩토리 등록
- **하드코딩 금지 포인트:**
  - 포맷 매직 바이트 → `detection.rs` 테이블로
  - 압축 레벨 매핑 → 포맷별 `CompressionProfile` 구조체
- **확장 지점은 테이블·트레잇으로.** `if/else` 체인 금지

---

## 10. 라이선스 (확정)

**오픈 코어 패턴:**
- **Rust 코어 (`crates/**`)**: `MIT OR Apache-2.0` (Rust 표준 듀얼)
- **App (`app/**`)**: Proprietary — All Rights Reserved

전체 전략·경계 규칙·심사 프로세스: [docs/01-plan/licensing.md](docs/01-plan/licensing.md)
제3자 고지 관리: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)

**PR 단계 규칙:**
- `crates/` ↔ `app/` 간 코드 이동은 재라이선스로 간주, 별도 승인 필요
- 새 의존성은 `cargo-deny` 화이트리스트 통과 필수 (GPL/AGPL/SSPL 거부)
- 기여는 `MIT OR Apache-2.0` 듀얼 인바운드 (Apache-2.0 §5) — PR 템플릿 체크박스

## 11. 외부 서비스 통합

### 11.1 크래시 리포팅 — Sentry (확정, 통합 시점은 후반)

- **도입 시점:** Phase 6~7 (UI 통합 이후, 공개 전). **MVP 구현 중에는 미통합** — 핵심 기능 완성 후 붙임.
- **SDK:** `Sentry.NET` (C#, AOT 호환 확인 필요 — `Sentry` 5.x 이상)
- **수집 대상:**
  - C# 미처리 예외
  - P/Invoke 경계에서 Rust → C# 로 올라온 패닉/에러
  - Rust 코어의 `tracing` 이벤트 중 `error!` 레벨 (FFI로 export 후 Sentry breadcrumb로 전달)
- **수집 금지 (PII/민감정보):**
  - 파일 경로는 익명화 (`%USERNAME%` → `<user>` 치환)
  - 아카이브 엔트리 이름 비전송
  - 패스워드는 `zeroize`로 이미 제거, 로그에도 절대 비포함
- **opt-in/opt-out:** 설정에서 사용자 동의 필수 (GDPR 대응). 기본값은 CI로 결정.
- **아키텍처 준비 (지금 해둘 것):**
  - `tracing` 크레이트를 Rust 코어에 미리 도입 → 나중에 Sentry breadcrumb subscriber만 붙이면 됨
  - C# `ILogger` 추상화 사용 → 나중에 Sentry Provider 추가만

### 11.2 자동 업데이트 — 불필요 (MS Store 배포)

- 배포 채널은 **Microsoft Store (MSIX)**
- 자동 업데이트·무결성 서명·Windows 샌드박싱·설치/제거 UX 모두 MS Store가 처리
- 자체 업데이터 구현 안 함

### 11.3 기타 외부 서비스

- **텔레메트리 (사용 통계):** 초기엔 없음. 필요 시 Sentry와 별도 서비스로 검토
- **라이선스 서버 (유료판):** 아직 없음. MS Store 구매 모델이면 MS가 관리

---

## 12. Phase 8 검증 매트릭스

| 정의 | Phase 8 검사 항목 |
|---|---|
| 네이밍 규칙 | `cargo clippy`, StyleCop 통과 |
| 폴더 구조 | 디렉터리 트리 diff |
| FFI 계약 | ABI 버전 검사, 헤더 diff CI |
| 성능 규약 | 벤치마크 회귀 게이트 |
| `unsafe` SAFETY | `clippy::undocumented_unsafe_blocks` |
| 의존 방향 | `cargo-modules`, C# 프로젝트 참조 그래프 |
