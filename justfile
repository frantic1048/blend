default:
    @just --list

# Build the release binary
build:
    cargo build --release --locked

# Build the debug binary
build-debug:
    cargo build --locked

# Type-check the package without building binaries
check:
    cargo check --locked

# Format Rust sources
fmt:
    cargo fmt

# Check formatting without modifying files
fmt-check:
    cargo fmt --check

# Run Clippy with warnings denied
clippy:
    cargo clippy --locked -- -D warnings

# Run the test suite
test:
    cargo test --locked

# Install website dependencies
website-install:
    cd website && pnpm install --frozen-lockfile

# Type-check and build the project website
website-check:
    cd website && pnpm check && pnpm build
