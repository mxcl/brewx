#!/usr/bin/env -S pkgx +gh +gum +npx +rustup +python@3.11 bash -exo pipefail

cd "$(dirname "$0")"/..

if ! git diff-index --quiet HEAD --; then
  echo "error: dirty working tree" >&2
  exit 1
fi

if [ "$(git rev-parse --abbrev-ref HEAD)" != main ]; then
  echo "error: requires main branch" >&2
  exit 1
fi

if test "$VERBOSE"; then
  set -x
fi

# Prevent AppleDouble (._) files and extended attributes in tar output.
export COPYFILE_DISABLE=1
export COPY_EXTENDED_ATTRIBUTES_DISABLE=1

cargo_toml="$PWD/Cargo.toml"
if [ ! -f "$cargo_toml" ]; then
  echo "error: $cargo_toml not found" >&2
  exit 1
fi

v_new="$(
  python - "$cargo_toml" <<'PY'
import sys
import tomllib

path = sys.argv[1]
with open(path, "rb") as f:
    data = tomllib.load(f)

package = data.get("package", {})
version = None
if isinstance(package, dict):
    version = package.get("version")

if not isinstance(version, str) or not version.strip():
    sys.stderr.write("error: version not found in Cargo.toml\n")
    sys.exit(1)

print(version.strip())
PY
)"

if [ "$(npx --yes -- semver "$v_new")" != "$v_new" ]; then
  echo "error: Cargo.toml version $v_new is not valid semver" >&2
  exit 1
fi

case $1 in
"")
  clobber=false
  ;;
clobber)
  clobber=true
  ;;
*)
  echo "usage $0 [clobber]" >&2
  exit 1
  ;;
esac

# ensure we have the latest version tags
git fetch origin -pft

# ensure github tags the right release
git push origin main

if versions="$(git tag | grep '^v[0-9]\+\.[0-9]\+\.[0-9]\+')"; then
  v_latest="$(npx --yes -- semver --include-prerelease $versions | tail -n1)"
fi

if [ -n "$v_latest" ]; then
  v_max="$(npx --yes -- semver --include-prerelease "$v_new" "$v_latest" | tail -n1)"
  if [ "$v_max" != "$v_new" ]; then
    echo "error: Cargo.toml version $v_new is older than latest tag v$v_latest" >&2
    exit 1
  fi
fi

if git tag -l "v$v_new" | grep -q .; then
  if [ "$clobber" = false ]; then
    echo "error: v$v_new already exists (use clobber to reupload)" >&2
    exit 1
  fi
fi

if [ "$clobber" = true ]; then
  true
elif ! gh release view v$v_new >/dev/null 2>&1; then
  gum confirm "prepare draft release for $v_new?" || exit 1

  gh release create \
    v$v_new \
    --draft=true \
    --generate-notes \
    $([ -n "$v_latest" ] && [ "$v_latest" != "$v_new" ] && echo "--notes-start-tag=v$v_latest") \
    --title=v$v_new
else
  gum format "> existing $v_new release found, using that"
  echo  # spacer
fi

target=aarch64-apple-darwin
artifact="brewx-$v_new-darwin-aarch64.tar.gz"

rustup target add "$target"

cargo build --release --target "$target"

strip "target/$target/release/brewx"

rm -f "$artifact"
tar -C "target/$target/release" -czf "$artifact" brewx

gh release upload --clobber v$v_new "$artifact"

gh release view v$v_new

if [ "$clobber" = false ]; then
  gum confirm "draft prepared, release $v_new?" || exit 1

  gh release edit \
    v$v_new \
    --verify-tag \
    --latest \
    --draft=false \
    --discussion-category=Announcements
fi

gh release view v$v_new --web
