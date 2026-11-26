# Agent instructions

Read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) before changing code. It covers
the build and checks, architecture, tests, CI, and release flow.

- Name development branches `dev/<topic>`, branched from `main`. Do not use
  tool- or agent-specific prefixes such as `codex/` or `claude/`.
- Run `just fmt-check`, `just clippy`, and `just test` before opening or
  updating a pull request.
