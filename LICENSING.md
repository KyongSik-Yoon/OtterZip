# OtterZip — License overview

OtterZip is open source, split into **two licensing areas**.

| Area | License |
|---|---|
| `crates/**` — Rust core | **MIT OR Apache-2.0** |
| `app/**` — Application (WinUI 3 / C# / C++ shell) | **GPL-3.0-or-later**, with an **UnRAR exception** |

---

## 1. Rust core (`crates/**`) — MIT OR Apache-2.0

Every source file in `crates/otterzip-core`, `crates/otterzip-ffi`,
`crates/otterzip-bench` and `crates/otterzip-cli` may be used under **either**:

- the **MIT License** — [`crates/LICENSE-MIT`](crates/LICENSE-MIT), or
- the **Apache License, Version 2.0** — [`crates/LICENSE-APACHE`](crates/LICENSE-APACHE)

SPDX identifier: `MIT OR Apache-2.0`

This is the standard Rust-ecosystem dual license, so the core stays reusable by
any Rust project. Apache-2.0 is also GPLv3-compatible, which is what lets the
GPL-licensed application link against this core.

### Contributions

Contributions intentionally submitted to the Rust core are, unless you state
otherwise, dual-licensed as above with no additional terms (Apache-2.0 §5).

---

## 2. Application (`app/**`) — GPL-3.0-or-later + UnRAR exception

`app/OtterZip.App`, `app/OtterZip.Interop`, `app/OtterZip.Shell` and
`app/OtterZip.App.Tests` are licensed under the **GNU General Public License,
version 3 or (at your option) any later version**.

- Full license text: [`COPYING`](COPYING)
- Application notice + the exception: [`app/LICENSE`](app/LICENSE)

### Why there is an UnRAR exception

OtterZip extracts RAR archives using UnRAR (© Alexander Roshal), reached through
the `unrar` / `unrar_sys` crates. **The UnRAR license is not GPL-compatible**: it
forbids using the UnRAR sources to develop or re-create the RAR *compression*
algorithm, and the GPL does not permit such extra restrictions to be imposed on
recipients.

So, as the copyright holder, LumiBear Studio grants an explicit exception
permitting the GPL parts to be linked with UnRAR and the combined work to be
distributed — the same approach 7-Zip takes ("GNU LGPL with unRAR restriction").
The exact wording is in [`app/LICENSE`](app/LICENSE).

**OtterZip only extracts RAR. It never creates RAR archives**, and the UnRAR code
here must not be used to develop or re-create the RAR compression algorithm.

Because of UnRAR, OtterZip as a whole is not "100% free software" by the
Debian/FSF definition, even though every line we wrote is.

---

## 3. Third-party components

Licenses for the third-party libraries shipped in the binary are listed in
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

Note in particular **UnRAR** (© Alexander Roshal) — see §2 above.

---

## 4. Distribution

The same application is distributed through the Microsoft Store and as a free
direct download. Both are built from this source. As the copyright holder,
LumiBear Studio may also distribute its own work under other terms; that does not
affect the rights this license grants you.

© 2026 LumiBear Studio
