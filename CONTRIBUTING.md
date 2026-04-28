# Contributing

Thank you for your interest in contributing to lmm!

## Development

```sh
# Build
cargo build

# Run tests
cargo test

# Lint
cargo clippy --all-targets -- -D warnings

# Format
cargo fmt
```

## Pull Requests

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Ensure `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` pass
4. Commit with a [gitmoji](https://gitmoji.dev/) prefix (e.g. `✨ Add new feature`)
5. Open a pull request

## Reporting Issues

Please open an issue with:
- Steps to reproduce
- Expected vs actual behavior
- `lmm --version` output
- macOS version and chip (e.g. M5 Max)

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
