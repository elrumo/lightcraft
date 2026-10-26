#!/bin/sh
# Builds the sync server's release on this Mac, from the committed code (HEAD), instead of in GitHub Actions
# (.github/workflows/server.yml, which stays the way to get the Windows zips). See docs/releasing.md.
#
#   tools/release-server.sh                       # builds the zips in dist/release/server (nothing leaves this Mac)
#   tools/release-server.sh --upload              # and puts them on a DRAFT GitHub release server-v<version>
#   tools/release-server.sh --upload --publish    # and publishes it (the release the macOS installer downloads)
#   tools/release-server.sh --docker              # and builds + pushes the Docker image (amd64 + arm64) to Docker Hub
#   --version 0.14.0   --image user/name (default elrumo/lightcraft-server)   --skip-linux   --skip-intel
#
# Builds: macOS arm64 and x86_64 (cargo, here), Linux x86_64 and aarch64 (cross-built in Docker, the image's own
# server stage). No Windows (needs a Windows toolchain: use the workflow). Needs: rust (+ wasm32 target and
# wasm-bindgen-cli, as for `cargo xtask web`), docker (OrbStack/Docker Desktop) for Linux and --docker, gh for --upload.
set -eu

version="" image=elrumo/lightcraft-server upload=0 publish=0 docker_push=0 linux=1 intel=1
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=$2; shift ;;
    --image) image=$2; shift ;;
    --upload) upload=1 ;;
    --publish) publish=1 ;;
    --docker) docker_push=1 ;;
    --skip-linux) linux=0 ;;
    --skip-intel) intel=0 ;;
    *) echo "unknown option $1 (see the top of this file)" >&2; exit 1 ;;
  esac
  shift
done
[ "$publish" = 0 ] || [ "$upload" = 1 ] || { echo "--publish needs --upload" >&2; exit 1; }

# this checkout has several remotes (origin, upstream): tell gh which repository the release is on
export GH_REPO=${GH_REPO:-elrumo/lightcraft}

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
[ -n "$version" ] || version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || { echo "not a version: $version" >&2; exit 1; }
commit=$(git rev-parse --short=10 HEAD)
tag="server-v$version"
out="$root/dist/release/server"

src=$(mktemp -d)
trap 'rm -rf "$src"' EXIT
git archive HEAD | tar -x -C "$src"
cp .dockerignore "$src"/ 2>/dev/null || true
rm -rf "$out" && mkdir -p "$out" "$src/_pkg"
echo "== LightCraft Server $version from commit $commit"

# the version and commit the admin page shows (there is no .git in the archive)
export LIGHTCRAFT_VERSION=$version LIGHTCRAFT_COMMIT=$commit
target_dir=${CARGO_TARGET_DIR:-$src/target}

echo "== web app"
(cd "$src" && cargo xtask web)
web="$src/_pkg/web"
mkdir -p "$web"
find "$target_dir/web-site" -maxdepth 1 -type f -exec cp {} "$web/" \;
test -f "$web/index.html"

package() { # package <system> <binary>
  name="lightcraft-server-$version-$1"
  d="$src/_pkg/$name"
  rm -rf "$d" && mkdir -p "$d"
  cp "$2" "$d/lightcraft-server" && chmod 755 "$d/lightcraft-server"
  cp -R "$web" "$d/web"
  cp LICENSE-MIT LICENSE-APACHE NOTICE "$d/"
  cp docs/self-hosting.md "$d/README-self-hosting.md"
  (cd "$src/_pkg" && ditto -c -k --keepParent "$name" "$out/$name.zip")
  echo "   $out/$name.zip"
}

mac() { # mac <rust target> <system>
  echo "== $2"
  rustup target add "$1" >/dev/null
  (cd "$src" && cargo build --release --locked -p lightcraft-server --target "$1")
  package "$2" "$target_dir/$1/release/lightcraft-server"
}
mac aarch64-apple-darwin macos-arm64
[ "$intel" = 0 ] || mac x86_64-apple-darwin macos-x86_64

# what the Mac that builds it can run: it starts, answers /health, serves the web app
if [ "$(uname -m)" = arm64 ]; then
  echo "== smoke test (macos-arm64)"
  exe="$target_dir/aarch64-apple-darwin/release/lightcraft-server"
  "$exe" --version
  data=$(mktemp -d)
  LIGHTCRAFT_DATA="$data" LIGHTCRAFT_LISTEN=127.0.0.1:18081 LIGHTCRAFT_WEB="$web" "$exe" serve >/dev/null 2>&1 &
  pid=$!
  ok=0
  for _ in $(seq 1 40); do "$exe" health --listen 127.0.0.1:18081 >/dev/null 2>&1 && ok=1 && break; sleep 1; done
  curl -fsS http://127.0.0.1:18081/ | grep -qi '<html' || ok=0
  kill $pid 2>/dev/null || true
  rm -rf "$data"
  [ "$ok" = 1 ] || { echo "the smoke test failed" >&2; exit 1; }
fi

if [ "$linux" = 1 ]; then
  echo "== linux (in Docker)"
  lin=$(mktemp -d)
  # the lite stage holds just the program: export its files and take it out
  docker buildx build --platform linux/amd64,linux/arm64 -f apps/lightcraft-server/Dockerfile --target lite \
    --build-arg "LIGHTCRAFT_VERSION=$version" --build-arg "LIGHTCRAFT_COMMIT=$commit" -o "type=local,dest=$lin" "$src"
  package linux-x86_64 "$lin/linux_amd64/usr/local/bin/lightcraft-server"
  package linux-aarch64 "$lin/linux_arm64/usr/local/bin/lightcraft-server"
  rm -rf "$lin"
fi

(cd "$out" && shasum -a 256 ./*.zip | sed 's| \./| |' > SHA256SUMS-server.txt && cat SHA256SUMS-server.txt)

if [ "$docker_push" = 1 ]; then
  echo "== Docker Hub: $image"
  minor=${version%.*}
  common="--platform linux/amd64,linux/arm64 -f apps/lightcraft-server/Dockerfile --build-arg LIGHTCRAFT_VERSION=$version --build-arg LIGHTCRAFT_COMMIT=$commit --label org.opencontainers.image.version=$version --label org.opencontainers.image.source=https://github.com/elrumo/lightcraft"
  case "$version" in
    *-*) lite_tags="-t $image:$version-lite"; full_tags="-t $image:$version" ;; # a prerelease: its own version only
    *) lite_tags="-t $image:$version-lite -t $image:$minor-lite -t $image:lite"; full_tags="-t $image:$version -t $image:$minor -t $image:latest" ;;
  esac
  # shellcheck disable=SC2086
  docker buildx build $common --target lite $lite_tags --push "$src"
  # shellcheck disable=SC2086
  docker buildx build $common $full_tags --push "$src"
fi

if [ "$upload" = 1 ]; then
  echo "== GitHub release $tag"
  git branch -r --contains HEAD | grep -q . || echo "   note: commit $commit is not pushed yet; the release's tag will point at the remote's default branch"
  mergedsums=$(mktemp)
  if gh release view "$tag" >/dev/null 2>&1; then
    # a release that exists (a draft, or Windows zips from the workflow): add to it, keeping the checksums of its other zips
    old=$(mktemp)
    if gh release download "$tag" -p SHA256SUMS-server.txt -O "$old" 2>/dev/null; then
      awk 'NR == FNR { new[$2] = 1; next } !($2 in new)' "$out/SHA256SUMS-server.txt" "$old" > "$mergedsums"
    fi
    rm -f "$old"
    cat "$out/SHA256SUMS-server.txt" >> "$mergedsums"
    sort -k2 "$mergedsums" > "$out/SHA256SUMS-server.txt"
    gh release upload "$tag" "$out"/* --clobber
  else
    notes="Self-hosted sync server. Docker: \`docker pull $image:$version\`. Mac: \`curl -fsSL https://raw.githubusercontent.com/elrumo/lightcraft/main/apps/lightcraft-server/install.sh | sh\`. Install guide: https://github.com/elrumo/lightcraft/blob/main/docs/self-hosting.md"
    flags="--draft"
    case "$version" in *-*) flags="$flags --prerelease" ;; esac
    # shellcheck disable=SC2086
    gh release create "$tag" "$out"/* $flags --title "LightCraft Server v$version" --notes "$notes"
  fi
  rm -f "$mergedsums"
  if [ "$publish" = 1 ]; then
    gh release edit "$tag" --draft=false
    echo "   published: https://github.com/elrumo/lightcraft/releases/tag/$tag"
  else
    echo "   draft created: https://github.com/elrumo/lightcraft/releases (open it and click Publish, or re-run with --publish)"
  fi
fi
echo "== done: $out"
