# Lawn

Lawn is a terminal workspace for focused local development, derived from
[herdr](https://github.com/herdrdev/herdr).

Version 0.1 is in development for macOS Apple Silicon. The workspace, Jump, and
parking changes are tracked in [issue #1](https://github.com/wkentaro/lawn/issues/1).

## Build

Use the Rust version in `rust-toolchain.toml`, Zig 0.16.0, `just`,
`cargo-nextest`, and Bun. If another Zig version is on your PATH, set `ZIG` to
the 0.16.0 executable.

```sh
just lawn-check
just build
./target/release/lawn
```

`lawn-check` runs formatting, clippy, all native Rust tests, maintenance tests,
integration-asset tests, and documentation contracts. The inherited `check`
recipe additionally requires the Windows SDK for cross-linting.

## Configuration

Release builds use `~/.config/lawn/config.toml`; debug builds use `lawn-dev`.
`XDG_CONFIG_HOME` changes the base directory, and `LAWN_CONFIG_PATH` selects an
explicit configuration file. Client presentation state uses Lawn's state
directory and respects `XDG_STATE_HOME`.

```sh
lawn --default-config
lawn --help
lawn completion zsh
```

Lawn retains the `HERDR_*` runtime, socket, and hook variables for compatibility.
An inherited socket override deliberately targets that session. Configuration
uses `LAWN_CONFIG_PATH` so a herdr configuration override does not silently
select Lawn's settings file.

## Upstream maintenance

This repository retains herdr's history and source paths. The starting revision
is `ee5bfb5041bb3e4a49d591c72308aae3c81ecdde`, whose runtime version is 0.9.5.
Lawn's release version is independent. Existing plugin manifests and the
inherited terminal identity and remote herdr bootstrap use `HERDR_COMPAT_VERSION` in
`src/build_info.rs`.

SSH machine setup downloads the matching upstream runtime. It does not copy
the local Lawn executable into a remote herdr installation.

Keep herdr as the `upstream` remote. Import selected revisions through reviewed
merge commits so their ancestry is preserved. Recheck runtime compatibility
when updating the upstream version, and keep Lawn release tags under `lawn-v*`.

## License

[Apache-2.0](LICENSE). Lawn preserves herdr's license and attribution notices.

Upstream sponsors and backers remain listed in [SPONSORS.md](SPONSORS.md).
