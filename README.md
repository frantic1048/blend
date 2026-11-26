# blend

Blend is a cross-platform dotfile manager for composing configuration in
[Nickel](https://nickel-lang.org/) and reconciling it with files used by
applications.

An `order.ncl` file declares one or more target files. Blend can copy literal
files and directories, or render structured Nickel values as TOML, JSON, JSONC,
YAML, and simple line-oriented formats. It compares structured files by value,
supports target-aware declarative policies, and falls back to interactive
resolution when the declaration does not decide a difference.

## Why Blend

- Keep platform logic in Nickel instead of embedding template syntax in target
  file formats.
- Mix structured `from_config` entries with faithful `from_file` copies.
- Describe ownership, defaults, enforcement, absence, and assertions close to
  the fields they affect.
- Preview changes and reconcile in either direction, with snapshot-backed
  Source/Target/Base context.
- Leave unresolved or application-owned state to an explicit interactive or
  unmanaged path.

Blend aims to satisfy declared constraints at best effort: automatically when
the declaration is authoritative, interactively when either side may be right,
and with a clear failure when an assertion or input cannot be satisfied safely.

## Install

### Homebrew

```sh
brew install frantic1048/tap/blend
```

### Installer

Blend 0.2.16 is the final release published from the Vanilla repository. This
standalone repository owns the 0.3.x release line and later, with release
archives published from
[frantic1048/blend](https://github.com/frantic1048/blend/releases). The
installer selects the requested platform archive, verifies its embedded SHA256
checksum, and installs to `~/.local/bin` by default:

```sh
curl -fsSLO https://github.com/frantic1048/Vanilla/releases/download/blend-v0.2.16/blend-installer.sh
sh blend-installer.sh
```

Use `sh blend-installer.sh --dir /path/to/bin` to choose another directory.
Published targets are:

- `aarch64-apple-darwin`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `x86_64-unknown-linux-gnu`

Linux release archives are built on Ubuntu 22.04 and support glibc 2.35 or
newer.

### Container

```sh
docker run --rm ghcr.io/frantic1048/blend:latest --version
docker run --rm \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/frantic1048/blend:latest check
```

The image entrypoint is `blend`. Version tags are multi-architecture images for
Linux AMD64 and ARM64. Stable releases also publish `latest`; prereleases
publish only their version tag.

### From source

```sh
git clone https://github.com/frantic1048/blend.git
cd blend
cargo build --release
```

The build writes the binary to `target/release/blend`.

## Quick start

Create a directory for Blend Source and initialize it:

```sh
mkdir dotfiles
cd dotfiles
blend init
```

`blend init` creates or refreshes the generated Order contract and metadata
defaults. It creates and deploys a starter Blend configuration only when
`orders/` is absent. Existing Source trees do not need an
`orders/blend/order.ncl` Order.

For precise configuration, author a Nickel Order directly. Create an Order:

```sh
blend create editor
```

Replace `orders/editor/order.ncl` with this complete structured TOML Order:

```nickel
let { Order, .. } = import "../order.contract.ncl" in
{
  blend = {
    prefix = ["~/.config/editor/"],
    files = [
      {
        name = "settings.toml",
        format = "toml",
        from_config =
          {
            theme = 'Enforce "dark",
          }
          |> blend.with_target_only 'Unmanaged,
      },
    ],
  },
} | Order
```

This enforces `theme` while retaining undeclared application-owned fields in
`~/.config/editor/settings.toml`. Preservation is semantic; structured output
may reformat the file. Use `theme = "dark"` for interactive reconciliation
instead of automatic enforcement.

Validate, preview, and reconcile:

```sh
blend check editor
blend view editor
blend sync editor
```

`check` and `view` are read-only. Review the preview before running `sync`, which
can prompt and write to either Source or Target; enforced fields can apply
without a prompt.

To import an existing Target as literal `from_file` Source instead:

```sh
blend add git ~/.config/git
```

`add` is an import convenience, not the structured Order authoring interface.
For agents, `blend skill print` prints the
[Order authoring skill](skills/blend-orders/SKILL.md) bundled with the installed
binary, offline and without a checkout or initialization. The
[user guide](docs/GUIDE.md#writing-an-order) lists release-specific copies.

Running `blend` without a subcommand shows deployment status. Dry-run output is
command-specific: `blend sync --dry-run` reports prospective reconciliation,
while `blend init --dry-run` only checks that generated Order files are current
and does not preview replacement content.

## Documentation

- [GUIDE.md](docs/GUIDE.md) — Orders, formats, resolution semantics, workflows,
  safety boundaries, and troubleshooting.
- [DEVELOPMENT.md](docs/DEVELOPMENT.md) — architecture, source map, tests, CI,
  and release maintenance.
- [DESIGN.md](docs/DESIGN.md) — durable design rationale, trade-offs, and scope.
- [CHANGELOG.md](CHANGELOG.md) — release history.

Versions through 0.2.x were developed and released from the
[Vanilla repository](https://github.com/frantic1048/Vanilla). This repository
is canonical from 0.3.0 onward, while Vanilla remains the primary real-world
consumer and integration corpus.
