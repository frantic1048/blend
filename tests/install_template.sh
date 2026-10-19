#!/bin/sh
set -eu

repo_root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT

archive_root="$test_root/archives"
build_root="$test_root/build"
fake_bin="$test_root/bin"
mkdir -p "$archive_root" "$build_root" "$fake_bin" "$test_root/home"

sha256_file() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	else
		shasum -a 256 "$1" | cut -d' ' -f1
	fi
}

make_archive() {
	target="$1"
	archive_dir="$build_root/blend-$target"
	mkdir -p "$archive_dir"
	printf '#!/bin/sh\nprintf "blend test %s\\n"\n' "$target" >"$archive_dir/blend"
	chmod +x "$archive_dir/blend"
	tar cJf "$archive_root/blend-$target.tar.xz" -C "$build_root" "blend-$target"
}

for target in \
	aarch64-apple-darwin \
	aarch64-unknown-linux-gnu \
	x86_64-apple-darwin \
	x86_64-unknown-linux-gnu; do
	make_archive "$target"
done

aarch64_apple_checksum="$(sha256_file "$archive_root/blend-aarch64-apple-darwin.tar.xz")"
aarch64_linux_checksum="$(sha256_file "$archive_root/blend-aarch64-unknown-linux-gnu.tar.xz")"
x86_64_apple_checksum="$(sha256_file "$archive_root/blend-x86_64-apple-darwin.tar.xz")"
x86_64_linux_checksum="$(sha256_file "$archive_root/blend-x86_64-unknown-linux-gnu.tar.xz")"

sed \
	-e 's/__APP_VERSION__/0.0.0-test/' \
	-e "s/__CHECKSUM_AARCH64_APPLE_DARWIN__/$aarch64_apple_checksum/" \
	-e "s/__CHECKSUM_AARCH64_UNKNOWN_LINUX_GNU__/$aarch64_linux_checksum/" \
	-e "s/__CHECKSUM_X86_64_APPLE_DARWIN__/$x86_64_apple_checksum/" \
	-e "s/__CHECKSUM_X86_64_UNKNOWN_LINUX_GNU__/$x86_64_linux_checksum/" \
	"$repo_root/.github/install_template.sh" >"$test_root/install.sh"

if grep -Eq '__APP_VERSION__|__CHECKSUM_[A-Z0-9_]+__' "$test_root/install.sh"; then
	printf 'installer still contains an unsubstituted placeholder\n' >&2
	exit 1
fi

cat >"$fake_bin/uname" <<'EOF'
#!/bin/sh
case "${1:-}" in
    -s) printf '%s\n' "$INSTALLER_TEST_OS" ;;
    -m) printf '%s\n' "$INSTALLER_TEST_ARCH" ;;
    *) exit 2 ;;
esac
EOF

cat >"$fake_bin/curl" <<'EOF'
#!/bin/sh
output=""
url=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o)
            shift
            output="$1"
            ;;
        https://*)
            url="$1"
            ;;
    esac
    shift
done

[ -n "$output" ] && [ -n "$url" ]
cp "$INSTALLER_TEST_ARCHIVES/${url##*/}" "$output"
EOF

chmod +x "$fake_bin/uname" "$fake_bin/curl"

test_install() {
	os="$1"
	arch="$2"
	target="$3"
	install_dir="$test_root/install-$os-$arch"

	INSTALLER_TEST_OS="$os" \
		INSTALLER_TEST_ARCH="$arch" \
		INSTALLER_TEST_ARCHIVES="$archive_root" \
		HOME="$test_root/home" \
		PATH="$install_dir:$fake_bin:$PATH" \
		sh "$test_root/install.sh" --quiet --dir "$install_dir"

	cmp "$build_root/blend-$target/blend" "$install_dir/blend"
}

test_install Darwin arm64 aarch64-apple-darwin
test_install Darwin x86_64 x86_64-apple-darwin
test_install Linux aarch64 aarch64-unknown-linux-gnu
test_install Linux arm64 aarch64-unknown-linux-gnu
test_install Linux x86_64 x86_64-unknown-linux-gnu

printf 'installer template supports every published target\n'
