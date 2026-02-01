#!/usr/bin/env -S pkgx +gh +gum +npx +rustup bash -exo pipefail

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

# ensure we have the latest version tags
git fetch origin -pft

# ensure github tags the right release
git push origin main

if versions="$(git tag | grep '^v[0-9]\+\.[0-9]\+\.[0-9]\+')"; then
  v_latest="$(npx --yes -- semver --include-prerelease $versions | tail -n1)"
fi

case $1 in
clobber)
  v_new=$v_latest
  ;;
major|minor|patch|prerelease)
  v_new=$(npx --yes -- semver bump $v_latest --increment $1)
  ;;
"")
  echo "usage $0 <major|minor|patch|prerelease|VERSION>" >&2
  exit 1;;
*)
  if test "$(npx --yes -- semver "$1")" != "$1"; then
    echo "$1 doesn't look like valid semver."
    exit 1
  fi
  v_new=$1
  ;;
esac

if [ $v_new = $v_latest ] && [ "$1" != clobber ]; then
  echo "$v_new already exists!" >&2
  exit 1
fi

if [ "$1" == clobber ]; then
  true
elif ! gh release view v$v_new >/dev/null 2>&1; then
  gum confirm "prepare draft release for $v_new?" || exit 1

  gh release create \
    v$v_new \
    --draft=true \
    --generate-notes \
    $([ -n "$v_latest" ] && echo "--notes-start-tag=v$v_latest") \
    --title=v$v_new
else
  gum format "> existing $v_new release found, using that"
  echo  # spacer
fi

target=aarch64-apple-darwin
artifact="brewx-$v_new-darwin-aarch64.tar.gz"

rustup target add "$target"

cargo build --release --target "$target"

rm -f "$artifact"
tar -C "target/$target/release" -czf "$artifact" brewx

gh release upload --clobber v$v_new "$artifact"

gh release view v$v_new

if [ "$1" != clobber ]; then
  gum confirm "draft prepared, release $v_new?" || exit 1

  gh release edit \
    v$v_new \
    --verify-tag \
    --latest \
    --draft=false \
    --discussion-category=Announcements
fi

gh release view v$v_new --web
