# Releasing TermDraft

This checklist publishes the pre-1.0 Rust application as the canonical `termdraft` executable.
The Python implementation remains a compatibility reference; native releases do not publish to
PyPI.

## Distribution boundary

- `Cargo.toml` is the authoritative package name and version source.
- Stable `vMAJOR.MINOR.PATCH` tags trigger `.github/workflows/release.yml`.
- The release workflow verifies Rust formatting, Clippy, debug tests, and release tests before
  building native archives for Linux and macOS on x86_64 and arm64.
- A successful workflow creates a draft GitHub release with four archives and `SHA256SUMS`.
- `Betancourt1/homebrew-tap` is updated only after the GitHub release is verified.
- The Python package under `src/termdraft` remains a compatibility oracle, not a release artifact.

The Cargo package is intentionally not published to crates.io. Supported installations are the
Homebrew formula, GitHub release archives, or `cargo install --path . --locked` from a checkout.

## Prepare the release commit

Start from a clean `main` synchronized with `origin/main`:

```bash
git status --short --branch
git fetch --prune --tags origin
git rev-parse main origin/main
```

Then:

1. set `package.version` in `Cargo.toml`;
2. move the release notes into a dated section in `CHANGELOG.md`;
3. update any versioned installation or compatibility language;
4. regenerate `Cargo.lock` if the package metadata changed;
5. confirm that the release tag does not already exist locally or remotely.

Run the complete local gates:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
cargo build --locked --release
./target/release/termdraft --version
./target/release/termdraft --help
./target/release/termdraft --commands
./target/release/termdraft --inspect .
.venv/bin/pytest -q
```

The Python suite is a regression check only. Do not build or upload Python wheels for a native tag.

Commit and push the release change, then wait for every hosted CI job on that exact commit. Local
success is not a substitute for hosted macOS and Linux verification.

## Tag and verify the draft

Create one annotated tag only after hosted CI passes:

```bash
git tag -a vX.Y.Z -m "TermDraft X.Y.Z"
git push origin vX.Y.Z
```

The tag starts the native release workflow. Wait for it to finish, then confirm that the draft
contains these assets:

```text
termdraft-X.Y.Z-aarch64-apple-darwin.tar.gz
termdraft-X.Y.Z-x86_64-apple-darwin.tar.gz
termdraft-X.Y.Z-aarch64-unknown-linux-gnu.tar.gz
termdraft-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz
SHA256SUMS
```

Download the draft assets into a new temporary directory, verify every checksum, unpack the archive
for the current machine, and rerun `--version`, `--help`, `--commands`, and `--inspect` from the
downloaded executable. Launch that executable in a real terminal against a disposable Markdown
fixture before publication.

Never move or reuse a public tag. If the workflow or smoke test fails, delete only the unpublished
draft and tag, fix the release commit, and choose a new patch version if the tag became public.

## Publish and update Homebrew

Publish the verified draft as the latest GitHub release. Then update
`Betancourt1/homebrew-tap/Formula/termdraft.rb` to the tagged source archive and its exact SHA-256.
The formula must build with Rust and install the `termdraft` executable.

Before pushing the tap change, run:

```bash
brew audit --strict --online Betancourt1/tap/termdraft
brew reinstall --build-from-source Betancourt1/tap/termdraft
brew test Betancourt1/tap/termdraft
termdraft --version
```

Finally, verify the published GitHub release, the tap commit, and a fresh Homebrew installation. The
release is complete only when all three point to the same TermDraft version.

## Rollback

Do not reuse a published GitHub release tag. A Homebrew regression is rolled back with a new tap
commit that restores the previous verified formula. The Python implementation remains available in
repository history and from its existing PyPI artifacts, but it is not republished as a native
release.
