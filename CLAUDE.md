# CLAUDE.md

Notes for AI coding agents working in this repository. People should start with the [README](README.md).

## Read first

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): the crates, the key rules, the quality gates and the open questions.
- [docs/GUI-DESIGN.md](docs/GUI-DESIGN.md): how the window looks and behaves, and why.
- [docs/RELEASING.md](docs/RELEASING.md): how a release is tagged, signed and published.

Read the part that covers what you're changing before you change it. If a change makes one of these wrong, fix the doc in the same pull request.

## Before opening a pull request

CI runs the first three on every push. The fuzz crate is outside the workspace and is only fuzzed once a change reaches `main`, so check it builds yourself.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --manifest-path fuzz/Cargo.toml
```

When dependencies change, also run `pwsh -File scripts/third-party-notices.ps1 -Check`. When the window changes, also run `pwsh -File scripts/check-gui.ps1`, which drives the real window through UI Automation.

## Conventions

- One branch per change, merged by rebase. `main` has no merge commits.
- Every change a user would notice gets a line under `## Unreleased` at the top of [CHANGELOG.md](CHANGELOG.md), written for users.
- A change to how rows are read, or to what the index stores, needs a new `MAGIC` in `crates/engine/src/cache.rs`, so indexes made by older versions are rebuilt.
- The engine has no `unsafe` code and no UI. Calls into Windows belong in `tv-platform`.
- Log files are untrusted input. New parsing code gets random-input tests, and a fuzz target where it reads bytes.
- Microsoft's message text is never copied into this repository. Error text is read from Windows or CMTrace at run time.
- Workflows pin their actions to exact commits.
- Don't commit screenshots or test logs that show a home folder path, a user name or a machine name.

## Releases

Tagging and publishing are the owner's decisions. Don't push a `v*` tag or publish a release unless asked to.
