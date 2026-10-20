# Contributing to Vigil

Thanks for helping out. Please read [CLAUDE.md](CLAUDE.md) for the repo rules and
[PLAN.md](PLAN.md) for the roadmap.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass; CI runs the same commands on Linux and macOS.

## Rules of thumb

- Conventional commits (`feat:`, `fix:`, `chore:`, `docs:`, `test:`), one logical change per commit.
- New pipeline steps, notification channels and API endpoints come with tests.
- No `unwrap()`/`expect()` outside tests and startup code.
- New dependencies need a one-line reason in `docs/decisions.md`; prefer pure-Rust crates.
- Never edit an applied SQL migration; add a new one.
- **Clean-room:** do not copy code, templates or text from changedetection.io.

By contributing you agree that your work is licensed under `MIT OR Apache-2.0`.
Please follow our [Code of Conduct](CODE_OF_CONDUCT.md).
