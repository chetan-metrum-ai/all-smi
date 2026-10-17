#!/usr/bin/env python3
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
"""Copy Shadeform validation logs to an artifact tree after redacting secrets/IPs."""

from __future__ import annotations

import argparse
import os
import re
from pathlib import Path

IPV4 = re.compile(
    rb"(?<![0-9A-Za-z_.])(?:(?:25[0-5]|2[0-4][0-9]|1?[0-9]?[0-9])\.){3}"
    rb"(?:25[0-5]|2[0-4][0-9]|1?[0-9]?[0-9])(?![0-9A-Za-z_.])"
)
PRIVATE_KEY = re.compile(
    rb"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?"
    rb"-----END [A-Z0-9 ]*PRIVATE KEY-----",
    re.DOTALL,
)
TEXT_SUFFIXES = {".raw", ".log", ".txt", ".json", ".exit"}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()

    args.destination.mkdir(parents=True, exist_ok=True)
    secret_values = [
        value.encode("utf-8")
        for value in (
            os.environ.get("SHADEFORM_API_KEY", ""),
            os.environ.get("SHADEFORM_SSH_PRIVATE_KEY", ""),
        )
        if value
    ]
    count = 0
    for source in sorted(args.source.rglob("*")):
        if not source.is_file() or source.suffix not in TEXT_SUFFIXES:
            continue
        data = source.read_bytes()
        for secret in secret_values:
            data = data.replace(secret, b"<redacted-secret>")
        data = PRIVATE_KEY.sub(b"<redacted-private-key>", data)
        data = IPV4.sub(b"<redacted-ip>", data)
        relative = source.relative_to(args.source)
        if relative.suffix == ".raw":
            relative = relative.with_suffix(".log")
        destination = args.destination / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        count += 1

    if count == 0:
        (args.destination / "validation.log").write_text(
            "No validation logs were produced before failure.\n", encoding="utf-8"
        )
        count = 1
    print(f"Prepared {count} redacted validation artifact file(s).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
