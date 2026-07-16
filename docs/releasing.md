# Verifying and publishing the Rust comparison

This document describes the `rust-port` branch. It is not the public TermDraft release checklist:
the existing GitHub release workflow, Python package, GitHub artifacts, and Homebrew formula still
publish the Python application from `main`.

Do not create a `vX.Y.Z` tag from this branch. The current release workflow interprets those tags as
Python releases and validates them against `pyproject.toml` and `src/termdraft/__init__.py`.

## Verify a Rust checkpoint

Start from a clean `rust-port` checkout with the committed lockfile:

```bash
git status --short --branch
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
cargo build --locked --release
```

Smoke-test the built binary without invoking Cargo again:

```bash
./target/release/termdraft-rs --version
./target/release/termdraft-rs --help
./target/release/termdraft-rs --commands
./target/release/termdraft-rs --inspect .
```

Then launch a disposable Markdown fixture in a real PTY and verify:

1. the title, Files pane, editor, and status line render;
2. `i` enters WRITE and `Esc` returns to COMMAND;
3. Unicode source can be typed and saved exactly;
4. `v` cycles Inline, Split, and Source without changing file bytes;
5. a dirty `q` requires an explicit Save, Discard, or Cancel choice;
6. the alternate screen, cursor, raw mode, and mouse reporting are restored after exit.

For isolated state during manual tests:

```bash
mkdir -p /tmp/termdraft-rs-fixture
printf '# QA\n\nCafé 日本語\n' > /tmp/termdraft-rs-fixture/note.md
XDG_STATE_HOME=/tmp/termdraft-rs-state \
  ./target/release/termdraft-rs \
  --config-dir /tmp/termdraft-rs-config \
  /tmp/termdraft-rs-fixture
```

The unchanged Python suite remains a compatibility oracle. If its development environment is
already prepared, run `pytest -q`; do not install or alter Python tooling merely to produce a Rust
binary.

## Current distribution boundary

The Rust comparison currently has:

- a branch-local Cargo package named `termdraft-rs`;
- one release-profile executable at `target/release/termdraft-rs`;
- no published crates.io package;
- no Rust GitHub release workflow or downloadable binary artifact;
- no Rust Homebrew formula;
- no published stable Rust release or Rust-specific tag namespace.

`cargo install --path . --locked` is the supported local installation path. It installs into the
user's Cargo bin directory and can be removed with `cargo uninstall termdraft-rs`.

## Before promoting Rust to a release

Promotion should be an explicit product decision rather than an incidental tag. At minimum:

1. Decide whether Rust replaces `termdraft` or remains `termdraft-rs`.
2. Resolve or formally accept the parity gaps in [RUST_PORT.md](../RUST_PORT.md).
3. Choose one authoritative version source and a tag namespace that cannot trigger the Python
   workflow accidentally.
4. Add Rust formatting, Clippy, tests, and builds to hosted CI on macOS and Linux.
5. Build release binaries on clean runners and verify their checksums and PTY startup.
6. Add a Rust-specific release workflow before publishing any tag.
7. Update Homebrew only after a public immutable artifact has passed installation and rollback
   tests.

The branch intentionally does not add those release mechanics yet; the goal is to compare the port,
not to replace the public distribution prematurely.

## Python release reference

The canonical 1.x [Python release guide](https://github.com/Betancourt1/TermDraft/blob/main/docs/releasing.md)
remains on `main`.

It updates `pyproject.toml` and `src/termdraft/__init__.py`, runs Ruff, mypy, pytest, build, and
Twine checks, publishes Python source/wheel artifacts, and then updates
`Betancourt1/homebrew-tap`. Run that process only from a clean `main` checkout.

## Rollback

A local Rust comparison has no remote artifact to roll back. Remove the installed executable with
`cargo uninstall termdraft-rs` or return to the previous branch commit. Never move or reuse a public
Python tag to represent a Rust build.
