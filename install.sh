#!/bin/sh
# Install perch from a GitHub release and wire it up.
#
#   curl -fsSL https://raw.githubusercontent.com/idossha/perch/main/install.sh | sh
#
# PERCH_VERSION=v0.2.0   pin a release instead of taking the latest
# PERCH_INSTALL_DIR=...  install somewhere other than ~/.local/bin
# --no-setup             install the binary only, do not run `perch setup`

set -eu

REPO="idossha/perch"
INSTALL_DIR="${PERCH_INSTALL_DIR:-$HOME/.local/bin}"
RUN_SETUP=1

for arg in "$@"; do
	case "$arg" in
	--no-setup) RUN_SETUP=0 ;;
	-h | --help)
		sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "install.sh: unknown argument: $arg" >&2
		exit 2
		;;
	esac
done

die() {
	echo "install.sh: $1" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

need tar
if command -v curl >/dev/null 2>&1; then
	fetch() { curl -fsSL "$1"; }
	download() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
	fetch() { wget -qO- "$1"; }
	download() { wget -qO "$2" "$1"; }
else
	die "curl or wget is required"
fi

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
Darwin) os_part="apple-darwin" ;;
Linux) os_part="unknown-linux-gnu" ;;
*) die "unsupported OS: $os" ;;
esac
case "$arch" in
x86_64 | amd64) arch_part="x86_64" ;;
arm64 | aarch64) arch_part="aarch64" ;;
*) die "unsupported architecture: $arch" ;;
esac
target="${arch_part}-${os_part}"

version="${PERCH_VERSION:-}"
if [ -z "$version" ]; then
	version="$(fetch "https://api.github.com/repos/${REPO}/releases/latest" |
		sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
	[ -n "$version" ] || die "could not determine the latest release; set PERCH_VERSION"
fi

name="perch-${version}-${target}"
url="https://github.com/${REPO}/releases/download/${version}/${name}.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "perch ${version} (${target})"
download "$url" "$tmp/perch.tar.gz" || die "download failed: $url"
tar xzf "$tmp/perch.tar.gz" -C "$tmp" || die "could not extract $name.tar.gz"

# Never replace a working install with something that is not there.
binary="$tmp/$name/perch"
[ -f "$binary" ] || binary="$(find "$tmp" -type f -name perch -perm -u+x | head -n 1)"
[ -n "$binary" ] && [ -f "$binary" ] || die "the tarball contained no perch binary"

mkdir -p "$INSTALL_DIR"
chmod +x "$binary"
mv -f "$binary" "$INSTALL_DIR/perch"
echo "installed $INSTALL_DIR/perch"

case ":$PATH:" in
*":$INSTALL_DIR:"*) ;;
*) echo "note: $INSTALL_DIR is not on your PATH" >&2 ;;
esac

if [ "$RUN_SETUP" -eq 1 ]; then
	"$INSTALL_DIR/perch" setup --yes
else
	echo "run '$INSTALL_DIR/perch setup' to wire perch into your harnesses"
fi
