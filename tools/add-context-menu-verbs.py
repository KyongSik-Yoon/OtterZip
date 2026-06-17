#!/usr/bin/env python3
# One-shot manifest mutation script for the 2026-05-19 Bandizip-parity sprint.
#
# Adds:
#   * 3 new Extract verbs (Smart, ToSubfolder, Dialog) to every existing
#     archive ItemType block that already references OtterzipExtract.
#   * 1 new Compress verb (CompressIndividually) to Type="*", "Directory",
#     and "Directory\Background" blocks.
#
# Idempotent — bails out if the new verbs are already present.

import re
import sys
from pathlib import Path

MANIFEST = Path(__file__).resolve().parent.parent / "app" / "OtterZip.App" / "Package.appxmanifest"

EXTRACT_VERB_LINES = """              <desktop5:Verb Id="OtterzipExtractSmart"
                             Clsid="77777777-2222-3333-4444-555555555555" />
              <desktop5:Verb Id="OtterzipExtractToSubfolder"
                             Clsid="88888888-2222-3333-4444-555555555555" />
              <desktop5:Verb Id="OtterzipExtractDialog"
                             Clsid="99999999-2222-3333-4444-555555555555" />
"""

COMPRESS_INDIVIDUAL_VERB_LINE = '              <desktop5:Verb Id="OtterzipCompressIndividually"\n                             Clsid="aaaaaaaa-2222-3333-4444-555555555555" />\n'

content = MANIFEST.read_text(encoding="utf-8")

if "OtterzipExtractSmart" in content:
    print("Manifest already updated — skipping")
    sys.exit(0)

# ---- 1. Inject the 3 extract verbs into every archive ItemType block --
# Each block currently has a single line like:
#   <desktop5:Verb Id="OtterzipExtract" Clsid="22222222-..." />
# We append the 3 new verbs right after that line.
extract_pattern = re.compile(
    r'(\s*<desktop5:Verb Id="OtterzipExtract"\s+Clsid="22222222-2222-3333-4444-555555555555" />\n)'
)
new_content, n_extract = extract_pattern.subn(
    lambda m: m.group(0) + EXTRACT_VERB_LINES,
    content,
)
print(f"Injected 3 extract verbs into {n_extract} archive ItemType blocks")

# ---- 2. Inject CompressIndividually into Type="*", Directory, Directory\Background.
# In each of those blocks, the verb appears AFTER the existing
# `OtterzipMenu` verb. We anchor on the closing ItemType tag and insert
# just before the OtterzipMenu line.
def inject_compress_individually(s: str) -> tuple[str, int]:
    # Find each <desktop5:ItemType Type="..."> ... </desktop5:ItemType> block
    # for compress-eligible types and insert the new verb. Anchor on the
    # OtterzipCompress verb (CLSID 4444) since it's only present in those
    # three blocks (Type="*", Directory, Directory\Background) and not in
    # archive blocks.
    anchor = re.compile(
        r'(\s*<desktop5:Verb Id="OtterzipCompress"\s+Clsid="44444444-2222-3333-4444-555555555555" />\n)'
    )
    new_s, n = anchor.subn(
        lambda m: m.group(0) + COMPRESS_INDIVIDUAL_VERB_LINE,
        s,
    )
    return new_s, n

new_content, n_compress = inject_compress_individually(new_content)
print(f"Injected CompressIndividually into {n_compress} non-archive ItemType blocks")

if n_extract == 0 or n_compress == 0:
    print("ERROR: at least one pattern matched zero times — manifest shape changed?", file=sys.stderr)
    sys.exit(1)

MANIFEST.write_text(new_content, encoding="utf-8")
print(f"Manifest written: {MANIFEST}")
