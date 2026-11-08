#!/usr/bin/env python3
"""Fail when an approved risk acceptance is past its expiry date.

Each .security/risk-acceptance/*.json acceptance has a companion .md record whose table holds
`| Status | ... |` and `| Expiry | YYYY-MM-DD |`. The JSON itself follows the audit tool's strict
schema (no status or expiry fields), so the Markdown table is authoritative.

A record whose Status contains "Approved" must carry an Expiry that is today or later (UTC).
Pending, superseded, and closed records are ignored.

Usage: check-risk-acceptance-expiry.py [DIR] [--today YYYY-MM-DD]
"""
from __future__ import annotations

import datetime
import pathlib
import re
import sys

ROW = re.compile(r"^\|\s*(Status|Expiry)\s*\|\s*(.*?)\s*\|\s*$")


def main(argv: list[str]) -> int:
    args = list(argv)
    today = datetime.datetime.now(datetime.timezone.utc).date()
    if "--today" in args:
        i = args.index("--today")
        today = datetime.date.fromisoformat(args[i + 1])
        del args[i : i + 2]
    root = pathlib.Path(args[0] if args else ".security/risk-acceptance")

    failures = []
    checked = 0
    for acceptance in sorted(root.glob("*.json")):
        record = acceptance.with_suffix(".md")
        if not record.is_file():
            failures.append(f"{acceptance}: no companion {record.name}")
            continue
        fields = {}
        for line in record.read_text(encoding="utf-8").splitlines():
            match = ROW.match(line)
            if match and match.group(1) not in fields:
                fields[match.group(1)] = match.group(2)
        status = fields.get("Status", "")
        if not re.search(r"\bApproved\b", status) or "Superseded" in status:
            continue
        checked += 1
        expiry_text = fields.get("Expiry", "")
        try:
            expiry = datetime.date.fromisoformat(expiry_text)
        except ValueError:
            failures.append(f"{record}: approved but Expiry {expiry_text!r} is not YYYY-MM-DD")
            continue
        if expiry < today:
            failures.append(f"{record}: approved acceptance expired on {expiry} (today {today})")

    for failure in failures:
        print(f"error: {failure}", file=sys.stderr)
    if failures:
        return 1
    print(f"risk acceptances: {checked} approved, none expired (today {today})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
