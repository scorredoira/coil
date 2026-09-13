#!/usr/bin/env python3
"""Reject binaries/grammars that require newer libraries than the Linux baseline.

Run in the release build container. Also load every grammar so missing dynamic
libraries or unresolved scanner symbols cannot slip through a --version check.
"""
import ctypes
from pathlib import Path
import re
import subprocess
import sys

LIMITS = {"GLIBC": (2, 28), "GLIBCXX": (3, 4, 25), "CXXABI": (1, 3, 11)}


def check(path):
    symbols = subprocess.check_output(["objdump", "-T", str(path)], text=True)
    for line in symbols.splitlines():
        if "*UND*" not in line:
            continue
        for family, version in re.findall(r"\b(GLIBC|GLIBCXX|CXXABI)_([0-9.]+)\b", line):
            if tuple(map(int, version.split("."))) > LIMITS[family]:
                raise RuntimeError("{} requires {}_{}".format(path, family, version))


def main():
    binary = Path(sys.argv[1]).resolve()
    directory = Path(sys.argv[2]).resolve()
    grammars = sorted(directory.glob("*.so"))
    if not grammars:
        raise RuntimeError("No compiled grammars to check")
    check(binary)
    subprocess.run([str(binary), "--version"], check=True)
    for grammar in grammars:
        check(grammar)
        ctypes.CDLL(str(grammar))
    print("Binary and {} grammars passed the glibc 2.28 compatibility check".format(len(grammars)))


if __name__ == "__main__":
    main()
