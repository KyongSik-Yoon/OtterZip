#!/usr/bin/env python3
"""Generate the hostile-filename RAR fixtures for `tests/rar_extract.rs`.

RAR is extract-only for us (RARLAB's licence forbids using the UnRAR source to
build a RAR-compatible archiver), so we cannot *write* a RAR to test our
extractor against. But we do not need to: the three fixtures below are the
shipped `version.rar` with its entry NAME patched in place and the header CRC
recomputed. Real unrar reads them back happily, which is the whole point --
these prove what our validation must survive, using unrar's own parser as the
judge.

Why this works without touching any other field:

  RAR3 file header (offset 20 in version.rar = 7-byte marker + 13-byte main
  header), with `head_size` = 39:

      +0  u16  HEAD_CRC     <- crc32(header[2..HEAD_SIZE]) & 0xFFFF
      +2  u8   HEAD_TYPE    0x74 = file header
      +3  u16  HEAD_FLAGS   0x8020 -- note LHD_UNICODE (0x200) is CLEAR, so
                            FILE_NAME is a raw byte string, not the packed
                            Unicode form. That is what makes a byte-for-byte
                            patch legal.
      +5  u16  HEAD_SIZE    39
      ...
      +7  u32  PACK_SIZE    21
      +11 u32  UNP_SIZE     11
      +25 u8   METHOD       0x33
      +26 u16  NAME_SIZE    7
      +28 u32  ATTR         0x81a4
      +32 [u8; NAME_SIZE]   b"VERSION"

  Every replacement name is therefore exactly 7 bytes (= len("VERSION")), which
  keeps NAME_SIZE, HEAD_SIZE and every downstream offset untouched. Only the
  name bytes and the 2-byte header CRC change.

Verified: the stored CRC of the pristine file (0x0c0f) reproduces exactly from
this formula, so the CRCs we write are computed the same way unrar checks them.

Run from the repo root:
    python tools/make-rar-fixtures.py
"""

import zlib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "crates" / "otterzip-core" / "tests" / "fixtures" / "rar"

HEADER_OFFSET = 20  # RAR3 file header in version.rar
ORIGINAL_NAME = b"VERSION"

# filename -> the 7-byte entry name to stamp in
NAME_CASES = {
    # `..` survives unrar's own cleaning -- our guard is the only defence.
    "traversal.rar": b"..\\..\\a",
    # RAR really does store backslashes; we must normalise, not reject.
    "backslash.rar": b"a\\b\\c.t",
    # unrar rewrites the drive colon to `_`, so our colon check is NOT what
    # saves us here -- the ParentDir rejection is. Documents the boundary.
    "absolute.rar": b"C:\\ev.t",
}

# Unix S_IFLNK | 0777. unrar derives FSREDIR_UNIXSYMLINK from exactly this
# high-nibble test (arcread.cpp:307-309), and `header_to_entry` reads the same
# bits, so this exercises the advisory symlink pre-filter in extract_all_inner
# without needing RARLAB's `rar -ol` to author a real link entry.
SYMLINK_ATTR = 0xA1FF

# A declared UNP_SIZE far above the 64 MiB threshold that `extract_entry` used
# to switch from `read()` (RAR_TEST -- creates nothing) to `extract_to`
# (RAR_EXTRACT -- writes the entry into %TEMP% and lets unrar prealloc it). The
# entry still holds its real 11 compressed bytes, so this is a header that LIES:
# exactly the shape that made the declared size an attacker-chosen disk cost.
# `read()` is bounded by the real data, so it returns the 11 bytes regardless.
BIG_UNP_SIZE = 100 * 1024 * 1024


def _fix_crc(out: bytearray) -> bytes:
    h = HEADER_OFFSET
    head_size = int.from_bytes(out[h + 5 : h + 7], "little")
    crc = zlib.crc32(out[h + 2 : h + head_size]) & 0xFFFF
    out[h : h + 2] = crc.to_bytes(2, "little")
    return bytes(out)


def patch_name(data: bytes, new_name: bytes) -> bytes:
    """Replace the file header's name and fix up HEAD_CRC."""
    h = HEADER_OFFSET
    out = bytearray(data)

    name_size = int.from_bytes(out[h + 26 : h + 28], "little")
    name_off = h + 32

    assert out[h + 2] == 0x74, "not a RAR3 file header"
    assert out[name_off : name_off + name_size] == ORIGINAL_NAME, "unexpected source name"
    assert len(new_name) == name_size, (
        "name must be exactly {} bytes to keep HEAD_SIZE valid, got {!r} ({} bytes)".format(
            name_size, new_name, len(new_name)
        )
    )

    out[name_off : name_off + name_size] = new_name
    return _fix_crc(out)


def patch_attr(data: bytes, attr: int) -> bytes:
    """Replace the file header's ATTR and fix up HEAD_CRC."""
    h = HEADER_OFFSET
    out = bytearray(data)
    assert out[h + 2] == 0x74, "not a RAR3 file header"
    out[h + 28 : h + 32] = attr.to_bytes(4, "little")
    return _fix_crc(out)


def patch_unp_size(data: bytes, unp_size: int) -> bytes:
    """Replace the file header's UNP_SIZE and fix up HEAD_CRC.

    Leaves PACK_SIZE and the packed bytes alone, so the header now claims far
    more output than the entry can possibly produce. The RAR3 unpacker takes
    UNP_SIZE as `DestUnpSize` and clamps its writes to it, so a lying value can
    only ever *over*-state the size -- which is what makes it a pure
    declared-size lie and not a corruption of the data stream.
    """
    h = HEADER_OFFSET
    out = bytearray(data)
    assert out[h + 2] == 0x74, "not a RAR3 file header"
    assert unp_size < 2**32, "UNP_SIZE is u32 without the LHD_LARGE flag"
    out[h + 11 : h + 15] = unp_size.to_bytes(4, "little")
    return _fix_crc(out)


def main() -> None:
    source = (FIXTURES / "version.rar").read_bytes()

    # Prove the formula against the pristine file before trusting it to write.
    h = HEADER_OFFSET
    head_size = int.from_bytes(source[h + 5 : h + 7], "little")
    stored = int.from_bytes(source[h : h + 2], "little")
    computed = zlib.crc32(source[h + 2 : h + head_size]) & 0xFFFF
    assert stored == computed, "CRC formula wrong: stored {:#06x} != computed {:#06x}".format(
        stored, computed
    )
    print("version.rar HEAD_CRC {:#06x} reproduces -- formula confirmed".format(stored))

    for filename, entry_name in NAME_CASES.items():
        (FIXTURES / filename).write_bytes(patch_name(source, entry_name))
        print("wrote {:<16} entry name = {!r}".format(filename, entry_name.decode("latin-1")))

    (FIXTURES / "symlink.rar").write_bytes(patch_attr(source, SYMLINK_ATTR))
    print("wrote {:<16} attr = {:#06x} (S_IFLNK)".format("symlink.rar", SYMLINK_ATTR))

    (FIXTURES / "bigdecl.rar").write_bytes(patch_unp_size(source, BIG_UNP_SIZE))
    print("wrote {:<16} UNP_SIZE = {} ({} MiB declared over 11 real bytes)".format(
        "bigdecl.rar", BIG_UNP_SIZE, BIG_UNP_SIZE // (1024 * 1024)
    ))


if __name__ == "__main__":
    main()
