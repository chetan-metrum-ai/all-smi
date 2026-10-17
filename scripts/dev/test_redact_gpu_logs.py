#!/usr/bin/env python3
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
"""Tests that uploaded Shadeform artifacts redact credentials and IPs."""

import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("redact-gpu-logs.py")
SPEC = importlib.util.spec_from_file_location("redact_gpu_logs", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
redactor = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(redactor)


class RedactGpuLogsTests(unittest.TestCase):
    def test_artifacts_redact_secrets_private_key_and_ipv4(self) -> None:
        api_key = "fake-shadeform-api-key"
        private_key = (
            "-----BEGIN OPENSSH PRIVATE KEY-----\n"
            "fake-private-key-material\n"
            "-----END OPENSSH PRIVATE KEY-----"
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "raw"
            destination = root / "safe"
            source.mkdir()
            (source / "run.raw").write_text(
                f"api={api_key}\nssh={private_key}\npeer=192.0.2.45\n",
                encoding="utf-8",
            )
            (source / "state.json").write_text(
                json.dumps({"ip": "198.51.100.20", "status": "active"}),
                encoding="utf-8",
            )
            (source / "key.raw").write_text(private_key, encoding="utf-8")
            (source / ".shadeform_ed25519").write_text(private_key, encoding="utf-8")

            with (
                patch.dict(
                    os.environ,
                    {
                        "SHADEFORM_API_KEY": api_key,
                        "SHADEFORM_SSH_PRIVATE_KEY": "fake-private-env-secret",
                    },
                ),
                patch.object(
                    sys,
                    "argv",
                    [str(MODULE_PATH), str(source), str(destination)],
                ),
            ):
                self.assertEqual(redactor.main(), 0)

            log = (destination / "run.log").read_text(encoding="utf-8")
            self.assertNotIn(api_key, log)
            self.assertNotIn("fake-private-key-material", log)
            self.assertNotIn("192.0.2.45", log)
            self.assertIn("<redacted-secret>", log)
            self.assertIn("<redacted-private-key>", log)
            self.assertIn("<redacted-ip>", log)
            state = json.loads((destination / "state.json").read_text(encoding="utf-8"))
            self.assertEqual(state["ip"], "<redacted-ip>")
            self.assertFalse((destination / ".shadeform_ed25519").exists())

            exact_destination = root / "exact-safe"
            with (
                patch.dict(
                    os.environ,
                    {
                        "SHADEFORM_API_KEY": api_key,
                        "SHADEFORM_SSH_PRIVATE_KEY": private_key,
                    },
                ),
                patch.object(
                    sys,
                    "argv",
                    [str(MODULE_PATH), str(source), str(exact_destination)],
                ),
            ):
                self.assertEqual(redactor.main(), 0)
            self.assertEqual(
                (exact_destination / "key.log").read_text(encoding="utf-8"),
                "<redacted-secret>",
            )


if __name__ == "__main__":
    unittest.main()
