#!/usr/bin/env python3
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
"""Unit tests for the Shadeform selector and CI credential source."""

import importlib.util
import io
import os
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("shadeform.py")
SPEC = importlib.util.spec_from_file_location("shadeform", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
shadeform = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(shadeform)


def instance(gpu_type: str, hourly_cents: object) -> dict[str, object]:
    return {
        "gpu_type": gpu_type,
        "num_gpus": 1,
        "cloud": "test-cloud",
        "shade_instance_type": f"{gpu_type}-instance",
        "configuration": {"os_options": ["ubuntu22.04_cuda13.0"]},
        "hourly_price": hourly_cents,
        "availability": [{"region": "test-region", "available": True}],
    }


class ShadeformTests(unittest.TestCase):
    def test_environment_api_key_is_used_without_env_json(self) -> None:
        with (
            patch.dict(os.environ, {"SHADEFORM_API_KEY": "  fake-test-value  "}),
            patch.object(shadeform, "ENV_PATH", Path("/definitely/missing/env.json")),
        ):
            self.assertEqual(shadeform.load_api_key(), "fake-test-value")

    def test_empty_environment_api_key_fails_closed(self) -> None:
        with patch.dict(os.environ, {"SHADEFORM_API_KEY": "  "}):
            with self.assertRaisesRegex(SystemExit, "environment variable is empty"):
                shadeform.load_api_key()

    def test_price_cap_skips_expensive_preferred_type_for_next_gpu(self) -> None:
        types = [
            instance("RTXPro6000", 301),
            instance("B200", 299),
            instance("H200", 150),
        ]
        selected = shadeform.pick_instance_type(types, "RTXPro6000", 3.0)
        self.assertEqual(selected["gpu_type"], "B200")
        self.assertEqual(selected["hourly_price_usd"], 2.99)

    def test_price_cap_rejects_unknown_prices(self) -> None:
        types = [instance("RTXPro6000", None), instance("H100", 200)]
        selected = shadeform.pick_instance_type(types, "RTXPro6000", 3.0)
        self.assertEqual(selected["gpu_type"], "H100")

    def test_price_cap_fails_when_no_candidate_is_below_limit(self) -> None:
        with self.assertRaisesRegex(SystemExit, "max_hourly_price_usd=3.0"):
            shadeform.pick_instance_type(
                [instance("RTXPro6000", 301), instance("H100", 400)],
                "RTXPro6000",
                3.0,
            )

    def test_price_cap_must_be_positive_and_finite(self) -> None:
        for cap in (0, -1, float("inf"), float("nan")):
            with self.subTest(cap=cap):
                with self.assertRaisesRegex(ValueError, "positive finite"):
                    shadeform.pick_instance_type([], "RTXPro6000", cap)

    def test_cli_rejects_invalid_price_cap_before_api_access(self) -> None:
        for value in ("0", "-1", "nan", "inf", "not-a-number"):
            with self.subTest(value=value), patch("sys.stderr", io.StringIO()):
                with self.assertRaises(SystemExit):
                    shadeform.build_parser().parse_args(
                        ["create", "--max-hourly-price-usd", value]
                    )


if __name__ == "__main__":
    unittest.main()
