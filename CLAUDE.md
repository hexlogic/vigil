# Vigil: rules for working in this repo

- Read PLAN.md before starting any phase. Follow the phase order.
- Before every commit: cargo fmt --all; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace.
- No `unwrap()`/`expect()` outside tests and `main` startup code. Use `thiserror` in library crates, `anyhow` in the binary.
- vigil-core is pure: no tokio, no network, no database.
- Every new pipeline step, notification channel or API endpoint ships with tests.
- Prefer small, focused modules (< 400 lines). Public items get doc comments.
- No new dependency without a one-line reason in docs/decisions.md. Prefer pure-Rust crates; no OpenSSL, no libxml2.
- Never edit an applied SQL migration; add a new one.
- Clean-room: do not copy code, templates or text from changedetection.io.
- Keep UI strings in locales/*.toml, never hardcode them in templates.
- Conventional commits. One logical change per commit.
