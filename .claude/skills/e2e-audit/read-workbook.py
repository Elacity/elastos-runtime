#!/usr/bin/env python3
"""Dump a sheet of the journey audit workbook as TSV, stdlib only.

Usage:
  read-workbook.py                     # list sheet names
  read-workbook.py <sheet>             # dump sheet as TSV
  read-workbook.py <sheet> --grep TXT  # only rows containing TXT (plus header)
"""
import re
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

M = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
WORKBOOK = Path(__file__).resolve().parents[3] / "docs/audits/ElastOS-Home-Journey-Audit.xlsx"


def main() -> int:
    z = zipfile.ZipFile(WORKBOOK)
    sheets = re.findall(r'<x:sheet name="([^"]+)" sheetId="\d+" r:id="([^"]+)"',
                        z.read("xl/workbook.xml").decode())
    rels = {}
    for m in re.finditer(r"<Relationship\b[^>]*>", z.read("xl/_rels/workbook.xml.rels").decode()):
        rid = re.search(r'Id="([^"]+)"', m.group(0))
        target = re.search(r'Target="([^"]+)"', m.group(0))
        if rid and target:
            rels[rid.group(1)] = target.group(1)

    if len(sys.argv) < 2:
        print("\n".join(name for name, _ in sheets))
        return 0

    wanted = sys.argv[1]
    grep = sys.argv[3] if len(sys.argv) > 3 and sys.argv[2] == "--grep" else None
    by_name = dict(sheets)
    if wanted not in by_name:
        print(f"unknown sheet {wanted!r}; sheets: {[n for n, _ in sheets]}", file=sys.stderr)
        return 2

    strings = ["".join(t.text or "" for t in si.iter(M + "t"))
               for si in ET.fromstring(z.read("xl/sharedStrings.xml"))]
    target = rels[by_name[wanted]].lstrip("/")
    if not target.startswith("xl/"):
        target = "xl/" + target
    root = ET.fromstring(z.read(target))

    for row in root.findall(f".//{M}sheetData/{M}row"):
        cells = []
        for c in row.findall(M + "c"):
            v = c.find(M + "v")
            if v is None:
                cells.append("")
            else:
                text = strings[int(v.text)] if c.get("t") == "s" else (v.text or "")
                cells.append(text.replace("\t", " ").replace("\n", " ⏎ "))
        line = "\t".join(cells).rstrip("\t")
        if not line.strip():
            continue
        if grep and grep.lower() not in line.lower():
            continue
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
