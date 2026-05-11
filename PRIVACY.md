# Privacy Policy / 개인정보 처리방침

**Effective date / 시행일:** 2026-05-11
**App:** OtterZip
**Publisher:** LumiBear Studio
**Contact:** nwlsrb@gmail.com · https://github.com/LumiBearStudio/OtterZip/issues

---

## English

### Summary

OtterZip is a desktop archive utility. It runs entirely on your computer. The
**only** information that ever leaves your device is an anonymous crash report
sent to Sentry, and **only when you explicitly turn that feature on**. It is
**off by default**.

We do not have user accounts, do not advertise, do not sell data, and do not
analyze your file contents.

### What we collect (only with your consent)

When you opt in to crash reporting in **Settings → Info → "Send anonymous
crash reports"**, OtterZip uses [Sentry](https://sentry.io) to collect:

- Stack traces from unhandled exceptions
- App version (e.g. 0.1.0)
- Operating system version
- CPU architecture (always x64 for OtterZip)
- A randomly generated install ID (per-installation, not per-user)

### What we do NOT collect

- **File paths or file names** — scrubbed from crash reports before they leave
  your device by a path-removal regex pass
- **File contents** of any archive you compress, extract, or browse
- **Passwords** used to encrypt or decrypt archives — these live in
  zero-on-drop memory and are stripped from any diagnostic string
- **Email addresses, IP addresses, or any account identifier** — OtterZip has
  no account system
- **Browser history, contacts, location, microphone, or camera data**

### Third-party processor

Crash reports (when enabled) are processed by **Functional Software, Inc.
d/b/a Sentry** (https://sentry.io). Their privacy policy:
https://sentry.io/privacy/. Reports are retained for the duration of Sentry's
default policy on our plan (currently 90 days), after which they are deleted.

### Your choices

- **Turn it off** at any time: Settings → Info → uncheck "Send anonymous
  crash reports". The change takes effect immediately.
- **Request deletion** of any prior crash data tied to your install ID by
  emailing nwlsrb@gmail.com with the install ID (visible in
  Settings → Info → Build).
- **Inspect what gets sent** by enabling Sentry's local debug mode via the
  environment variable `OTTERZIP_TELEMETRY_DEBUG=1` before launching.

### Children

OtterZip is not directed to children under 13 (or 16 in the EU). We do not
knowingly collect personal data from children.

### Changes to this policy

If we change what is collected, we will update this document and bump the
effective date above. Material changes will also be announced in the GitHub
release notes for the next version.

---

## 한국어

### 요약

OtterZip은 컴퓨터에서 완전히 로컬로 동작하는 압축/해제 도구입니다. 기기를
떠나는 정보는 **익명 크래시 리포트** 단 한 가지뿐이며, 그조차도 사용자가
**명시적으로 켰을 때만** 전송됩니다. **기본값은 OFF**입니다.

사용자 계정 시스템이 없고, 광고를 표시하지 않으며, 데이터를 판매하지
않습니다. 사용자의 파일 내용은 절대 분석하지 않습니다.

### 수집 항목 (사용자 동의 시에만)

**설정 → 정보 → "익명 크래시 리포트 전송"** 에서 켜셨을 때, OtterZip은
[Sentry](https://sentry.io)를 통해 다음을 수집합니다.

- 처리되지 않은 예외의 스택 트레이스
- 앱 버전 (예: 0.1.0)
- 운영체제 버전
- CPU 아키텍처 (OtterZip은 항상 x64)
- 무작위로 생성된 설치 ID (사용자가 아닌 설치 단위)

### 수집하지 않는 것

- **파일 경로 / 파일 이름** — 기기를 떠나기 전 정규식 기반 스크러빙으로 제거
- **압축 / 해제 / 열람한 아카이브의 내용**
- **암호** — 메모리에서 zero-on-drop 처리되며 진단 문자열에서도 제거됨
- **이메일, IP 주소, 계정 식별자** — OtterZip은 계정 시스템 자체가 없음
- **브라우저 기록, 연락처, 위치, 마이크, 카메라**

### 제3자 처리자

크래시 리포트(활성화 시)는 **Functional Software, Inc. d/b/a Sentry**
(https://sentry.io)에서 처리됩니다. Sentry 개인정보 처리방침:
https://sentry.io/privacy/. 리포트는 현재 가입한 Sentry 요금제의 기본 보존
정책(현재 90일)에 따라 보관 후 삭제됩니다.

### 사용자 권리

- **언제든지 끌 수 있음**: 설정 → 정보 → "익명 크래시 리포트 전송" 체크
  해제. 즉시 반영됩니다.
- **이전 데이터 삭제 요청**: 설치 ID(설정 → 정보 → 빌드에서 확인 가능)와
  함께 nwlsrb@gmail.com 으로 이메일 부탁드립니다.
- **전송 내용 확인**: 실행 전 환경변수 `OTTERZIP_TELEMETRY_DEBUG=1` 설정 시
  Sentry 로컬 디버그 모드가 활성화됩니다.

### 아동 정책

OtterZip은 만 13세(EU 기준 만 16세) 미만 아동을 대상으로 하지 않으며,
아동의 개인정보를 의도적으로 수집하지 않습니다.

### 정책 변경

수집 항목을 변경하는 경우 이 문서와 상단의 시행일을 업데이트합니다.
중대한 변경은 다음 버전의 GitHub 릴리스 노트에서도 공지됩니다.

---

## Quick reference table

| Question | Answer |
|---|---|
| Does the app phone home by default? | **No.** Telemetry is OFF by default. |
| Is there a user account / login? | **No.** OtterZip is fully local. |
| Are file contents scanned or uploaded? | **Never.** |
| Are file names sent in crash reports? | **No** — scrubbed before send. |
| Are passwords sent in crash reports? | **No** — zero-on-drop + scrubbed. |
| Who is the third-party data processor? | Sentry (https://sentry.io) |
| How do I opt out? | Settings → Info → uncheck the toggle. |
| How do I request data deletion? | Email nwlsrb@gmail.com with your install ID. |
