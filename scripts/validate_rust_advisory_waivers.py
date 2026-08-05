#!/usr/bin/env python3
"""Validate that every cargo-deny RustSec waiver is temporary and auditable."""

from __future__ import annotations

import argparse
from collections.abc import Mapping
from datetime import date, datetime, timezone
from pathlib import Path
import re
import sys
import tomllib

ADVISORY_ID_PATTERN = re.compile(r"^RUSTSEC-\d{4}-\d{4}$")
REASON_PATTERN = re.compile(
    r"^expires=(?P<expires>\d{4}-\d{2}-\d{2}); "
    r"tracking=(?P<tracking>https://\S+); "
    r"(?P<rationale>\S(?:.*\S)?)$"
)
MAX_WAIVER_DAYS = 90


class WaiverValidationError(ValueError):
    """Raised when advisory waiver policy is invalid."""


def validate_waivers(config: Mapping[str, object], *, today: date) -> int:
    """Validate cargo-deny advisory ignores and return the waiver count."""
    advisories = config.get("advisories")
    if not isinstance(advisories, Mapping):
        raise WaiverValidationError("deny.toml must contain an [advisories] table")

    if "ignore" not in advisories:
        raise WaiverValidationError("[advisories].ignore must be declared explicitly")

    waivers = advisories["ignore"]
    if not isinstance(waivers, list):
        raise WaiverValidationError("[advisories].ignore must be an array")

    seen_ids: set[str] = set()
    for position, waiver in enumerate(waivers, start=1):
        label = f"advisory waiver #{position}"
        if not isinstance(waiver, Mapping):
            raise WaiverValidationError(
                f"{label} must be a table with id and reason fields"
            )
        if set(waiver) != {"id", "reason"}:
            raise WaiverValidationError(
                f"{label} must contain exactly the id and reason fields"
            )

        advisory_id = waiver["id"]
        reason = waiver["reason"]
        if not isinstance(advisory_id, str) or not ADVISORY_ID_PATTERN.fullmatch(
            advisory_id
        ):
            raise WaiverValidationError(f"{label} has an invalid RustSec advisory ID")
        if advisory_id in seen_ids:
            raise WaiverValidationError(f"duplicate advisory waiver: {advisory_id}")
        seen_ids.add(advisory_id)

        if (
            not isinstance(reason, str)
            or (match := REASON_PATTERN.fullmatch(reason)) is None
        ):
            raise WaiverValidationError(
                f"{advisory_id} reason must match: "
                "expires=YYYY-MM-DD; tracking=https://...; rationale"
            )

        try:
            expiry = date.fromisoformat(match.group("expires"))
        except ValueError as error:
            raise WaiverValidationError(
                f"{advisory_id} has an invalid expiry date"
            ) from error

        remaining_days = (expiry - today).days
        if remaining_days < 0:
            raise WaiverValidationError(
                f"{advisory_id} expired on {expiry.isoformat()}"
            )
        if remaining_days > MAX_WAIVER_DAYS:
            raise WaiverValidationError(
                f"{advisory_id} expires in {remaining_days} days; "
                f"the maximum is {MAX_WAIVER_DAYS}"
            )

    return len(waivers)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path, help="path to deny.toml")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        with args.config.open("rb") as config_file:
            config = tomllib.load(config_file)
        today = datetime.now(timezone.utc).date()
        count = validate_waivers(config, today=today)
    except (OSError, tomllib.TOMLDecodeError, WaiverValidationError) as error:
        print(f"advisory waiver validation failed: {error}", file=sys.stderr)
        return 1

    noun = "waiver" if count == 1 else "waivers"
    print(f"validated {count} temporary advisory {noun}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
