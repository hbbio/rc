#!/usr/bin/env python3
"""Tests for the RustSec waiver policy validator."""

from __future__ import annotations

from datetime import date
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from validate_rust_advisory_waivers import (  # noqa: E402
    WaiverValidationError,
    validate_waivers,
)


TODAY = date(2026, 8, 5)


def config_with(*waivers: object) -> dict[str, object]:
    return {"advisories": {"ignore": list(waivers)}}


def waiver(
    advisory_id: str = "RUSTSEC-2026-0001",
    *,
    expires: str = "2026-09-01",
) -> dict[str, str]:
    return {
        "id": advisory_id,
        "reason": (
            f"expires={expires}; tracking=https://example.invalid/issues/1; "
            "upgrade is blocked upstream"
        ),
    }


class ValidateWaiversTests(unittest.TestCase):
    def test_accepts_an_explicit_empty_waiver_list(self) -> None:
        self.assertEqual(validate_waivers(config_with(), today=TODAY), 0)

    def test_accepts_a_tracked_unexpired_waiver(self) -> None:
        self.assertEqual(validate_waivers(config_with(waiver()), today=TODAY), 1)

    def test_rejects_unstructured_string_waivers(self) -> None:
        with self.assertRaisesRegex(WaiverValidationError, "must be a table"):
            validate_waivers(config_with("RUSTSEC-2026-0001"), today=TODAY)

    def test_rejects_expired_waivers(self) -> None:
        with self.assertRaisesRegex(WaiverValidationError, "expired"):
            validate_waivers(
                config_with(waiver(expires="2026-08-04")), today=TODAY
            )

    def test_rejects_waivers_longer_than_ninety_days(self) -> None:
        with self.assertRaisesRegex(WaiverValidationError, "maximum is 90"):
            validate_waivers(
                config_with(waiver(expires="2026-11-04")), today=TODAY
            )

    def test_rejects_duplicate_advisories(self) -> None:
        with self.assertRaisesRegex(WaiverValidationError, "duplicate"):
            validate_waivers(config_with(waiver(), waiver()), today=TODAY)

    def test_rejects_invalid_identifiers_and_reasons(self) -> None:
        invalid_waivers = (
            {**waiver(), "id": "GHSA-xxxx-yyyy-zzzz"},
            {**waiver(), "reason": "waiting for an upstream release"},
            waiver(expires="2026-02-30"),
        )
        for invalid_waiver in invalid_waivers:
            with self.subTest(waiver=invalid_waiver):
                with self.assertRaises(WaiverValidationError):
                    validate_waivers(config_with(invalid_waiver), today=TODAY)

    def test_accepts_a_waiver_on_its_expiry_date(self) -> None:
        self.assertEqual(
            validate_waivers(config_with(waiver(expires="2026-08-05")), today=TODAY),
            1,
        )

    def test_requires_the_advisories_table_and_ignore_array(self) -> None:
        invalid_configs = ({}, {"advisories": {}}, {"advisories": {"ignore": ""}})
        for config in invalid_configs:
            with self.subTest(config=config):
                with self.assertRaises(WaiverValidationError):
                    validate_waivers(config, today=TODAY)


if __name__ == "__main__":
    unittest.main()
