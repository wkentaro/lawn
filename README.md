# Lawn

Lawn is a terminal workspace for focused local development, derived from
[herdr](https://github.com/herdrdev/herdr).

Version 0.1 is in development for macOS Apple Silicon. The workspace, Jump, and
parking changes are tracked in [issue #1](https://github.com/wkentaro/lawn/issues/1).

## Install a release

After a release is published, download and verify its artifacts before installing:

```sh
lawn_download=$(mktemp -d) &&
gh release download lawn-v0.1.0 --repo wkentaro/lawn --dir "$lawn_download" &&
(cd "$lawn_download" && shasum -a 256 -c SHA256SUMS &&
  install -d "$HOME/.local/bin" &&
  install -m 755 lawn-macos-aarch64 "$HOME/.local/bin/lawn")
```

Keep `~/.local/bin` on your `PATH`. The archive also contains the executable,
license, and upstream attribution. `lawn update` uses Lawn's release manifest.

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

## Park and unpark

Right-click a workspace pane and choose **Park** to hide it in Lawn.
The terminal keeps running. **Parked** uses the same machine and workspace order
as Jump; selecting a pane there opens a read-only preview. Choose **Unpark**
in the preview header or the pane's context menu to resume input.

Parking works for agents and ordinary terminals, including long-running jobs.
Parked panes disappear from active Jump results and tab indicators. A tab with
all its panes parked disappears until a pane is unparked. Lawn keeps you in
the workspace when its last visible pane is parked.

These shortcuts have no defaults. Unpark keeps its existing `restore_pane`
configuration key, and `open_later` still works as an alias for `open_parked`:

```toml
[keys]
park_pane = "prefix+f"
restore_pane = "prefix+u"
open_parked = "ctrl+g"
```

The menu also opens Parked when the sidebar is hidden. Parking state is saved
locally per server and shared by Lawn windows on this computer. A preview stays
read-only until you unpark it in that window, even if another window has
already unparked it. Other herdr clients keep their own layouts.

Parking requires a runtime advertising `pane_presentation_v1`, as provided by
Lawn's bundled runtime. Older herdr servers retain normal terminal operation;
parking is unavailable there. Lawn's inherited SSH bootstrap currently installs
the upstream runtime, which may not advertise this extension.

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

## Release preparation

Build a local candidate and package it into a new directory:

```sh
just lawn-check
just build
python3 scripts/lawn_release.py --binary target/release/lawn \
  --output artifacts --tag lawn-v0.1.0
```

The packager checks the binary's version and Apple Silicon architecture and
includes checksums, an archive, and the updater manifest. Keep release notes in
`docs/lawn/releases/<version>.md` aligned with the candidate.

The Lawn release workflow builds artifacts on manual dispatch and packaging PRs.
Publication requires a matching `lawn-v<version>` tag on a commit already in
`main`, passing checks, and repository admin permission. Merge the reviewed
stack and verify the installed candidate before creating that tag.

## License

[Apache-2.0](LICENSE). Lawn preserves herdr's license and attribution notices.

Upstream sponsors and backers remain listed in [SPONSORS.md](SPONSORS.md).
