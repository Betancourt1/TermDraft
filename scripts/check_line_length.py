"""Reject tracked text lines longer than the project limit."""

from __future__ import annotations

import subprocess
from pathlib import Path

MAX_LINE_LENGTH = 100


def tracked_files() -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        check=True,
        stdout=subprocess.PIPE,
    )
    return [Path(path) for path in result.stdout.decode().rstrip("\0").split("\0") if path]


def main() -> int:
    violations: list[str] = []
    for path in tracked_files():
        content = path.read_bytes()
        if b"\0" in content:
            continue
        for line_number, line in enumerate(content.decode("utf-8").splitlines(), start=1):
            if len(line) > MAX_LINE_LENGTH:
                violations.append(f"{path}:{line_number}: {len(line)} characters")

    if violations:
        print("Lines exceed 100 characters:")
        print("\n".join(violations))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
