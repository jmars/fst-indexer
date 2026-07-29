# Contributing to fst-indexer

Thanks for considering contributing! All types of contributions — bug reports, feature ideas, documentation improvements, and code changes — are welcome.

## Setup

1. **Clone the repository:**

   ```bash
   git clone https://github.com/jmars/fst-indexer.git
   cd fst-indexer
   ```

2. **Prerequisites:** Install the Rust toolchain via [rustup](https://rustup.rs/). The project uses the 2021 edition and builds with stable Rust.

3. **Build:**

   ```bash
   cargo build
   ```

## Development Workflow

Run these checks before every commit:

```bash
cargo fmt --check     # Ensure consistent formatting
cargo clippy -- -D warnings  # Lint with no warnings
cargo test            # Run all tests
```

If `cargo fmt --check` fails, fix formatting with:

```bash
cargo fmt
```

## Testing

Tests live in `src/lib.rs` under `#[cfg(test)] mod tests`. They use `tempfile::tempdir()` to create temporary directories, build indexes from sample data, and then search them to verify correctness. There are 14 tests covering:

- Tokenization (basic, short-word filtering, hyphen/underscore)
- Date extraction from filenames
- Build + search round-trips (AND and OR modes)
- Empty queries, no-match queries, out-of-range resolution
- Error handling (nonexistent directories)
- JSONL extractor (content field and raw-line fallback)

To run a specific test:

```bash
cargo test test_build_and_search
```

## Pull Request Process

1. **Open an issue first** for feature requests or significant changes to discuss the design before writing code.
2. **Reference the issue** in your PR description using `Closes #123` or `Refs #123`.
3. **Keep PRs focused** — one feature or fix per PR. Small, targeted PRs are reviewed faster.
4. Ensure all CI checks pass (formatting, clippy, tests).
5. Update documentation (README or doc comments) if your change affects the public API or CLI.

## Code Style

- Follow standard Rust conventions as enforced by `rustfmt` and `clippy`.
- Keep the code clippy-clean with no warnings (`-D warnings`).
- Use `anyhow::Result` for fallible functions.
- Public API types and methods should have doc comments.

## License

This project is licensed under the [MIT License](LICENSE). By contributing, you agree that your contributions will be licensed under the same license.
