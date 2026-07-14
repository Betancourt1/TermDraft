# Releasing TermDraft

This is the maintainer checklist for a public release. Complete it from a clean `main` checkout and
keep release preparation separate from feature work.

## 1. Prepare the release

1. Confirm `main` is clean, up to date, and passing required CI.
2. Choose the version and confirm the `termdraft` distribution remains available on every intended
   package channel.
3. Update the same version in both locations:
   - `pyproject.toml` under `[project].version`;
   - `src/termdraft/__init__.py` as `__version__`.
4. Move the relevant entries from `[Unreleased]` in `CHANGELOG.md` under the new version heading.
5. Run the local gates:

   ```bash
   pytest -q
   ruff format --check .
   ruff check .
   mypy
   python -m build
   python -m twine check --strict dist/*
   ```

   Install `build` and `twine` in the release environment if needed. Build from a clean checkout so
   `dist/` contains only artifacts for the intended version.
6. Install the wheel into a fresh virtual environment and smoke-test `termdraft --version`,
   `termdraft --help`, `termdraft --commands`, and `python -m termdraft --version`.

## 2. Tag and publish

1. Commit the version and changelog together.
2. Create an annotated tag:

   ```bash
   git tag -a vX.Y.Z -m "TermDraft X.Y.Z"
   ```

3. Push `main`, then push the tag.
4. Inspect the draft GitHub release before publishing it. Confirm the source distribution and wheel
   names, SHA-256 checksums, release notes, and macOS/Linux installation smoke results.
5. Publish the GitHub release only after every artifact and check matches the tagged commit.

## 3. Update Homebrew after publication

The first-party formula belongs in a separate `OWNER/homebrew-tap` repository. Keeping the tap
separate preserves the application repository's source history while giving Homebrew formula
changes, audits, and rollbacks their own small history.

1. Update the formula from the public source distribution URL, not a local artifact or draft URL.
2. Copy the public source distribution's SHA-256 checksum into the formula.
3. Commit and push the formula change in `OWNER/homebrew-tap`.
4. Test a clean install, the command, and removal:

   ```bash
   brew install OWNER/tap/termdraft
   termdraft --version
   brew uninstall termdraft
   ```

5. When a prior formula exists, install that version first, publish the update, then verify:

   ```bash
   brew update
   brew upgrade OWNER/tap/termdraft
   termdraft --version
   brew uninstall termdraft
   ```

For 1.0, update the tap manually. Do not add automatic cross-repository updates until the public
artifact, checksum, formula, and rollback workflow has succeeded end to end.

## Rollback

- Leave a failed draft release unpublished, or delete the draft, then fix the problem and release a
  new version.
- Never move, replace, or reuse a published tag. Any correction after publication gets a newer
  version and a new tag.
- If a Homebrew formula is broken, revert or supersede the formula in the tap while preparing the
  fixed release; do not silently replace an artifact or checksum for an existing version.
