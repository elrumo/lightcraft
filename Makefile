# Build the sync server image and push it to the registry (docs/self-hosting.md).
#
#   make push-server                 # amd64 + arm64, from the working tree, next patch version
#   make push-server REF=HEAD        # from the committed code only (ignores uncommitted work)
#   make push-server BUMP=minor      # or major; VERSION=1.0.0-rc.1 sets the version outright
#   make server-version              # the version the next push would get (pushes nothing)
#
# Every push is tagged with its version (semver, 0.2.3) and `latest`, and carries the version in its
# org.opencontainers.image.version label: the next push reads it from :latest and bumps it. A version
# is never below the workspace version in Cargo.toml, so the image follows the app's releases, and an
# existing version is never overwritten.
#
# Needs `docker login registry.eliasruiz.com` once, and a buildx that can push multi-platform
# (Docker Desktop / OrbStack with the containerd image store).

IMAGE     ?= registry.eliasruiz.com/library/lightcraft
PLATFORMS ?= linux/amd64,linux/arm64
REF       ?=
BUMP      ?= patch
VERSION   ?=

# Shell that prints the next version, or fails with a message. A variable, not a target run with
# $(MAKE): make runs $(MAKE) lines even under `make -n`, which would push. No `case` in it: macOS's
# /bin/sh (bash 3.2) misparses a case pattern's `)` inside push-server's $( ).
next_version = set -e; \
	if [ -n "$(VERSION)" ]; then v="$(VERSION)"; else \
	  echo "$(BUMP)" | grep -Eqx 'major|minor|patch' || { echo "BUMP must be major, minor or patch" >&2; exit 1; }; \
	  floor=$$($(if $(REF),git show $(REF):Cargo.toml,cat Cargo.toml) \
	    | sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([0-9]*\.[0-9]*\.[0-9]*\).*/\1/p'); \
	  last=$$(docker buildx imagetools inspect $(IMAGE):latest --format '{{json .Image}}' 2>/dev/null \
	    | sed -n 's/.*"org.opencontainers.image.version": *"\([0-9]*\.[0-9]*\.[0-9]*\).*/\1/p' | head -1); \
	  v=$$(echo "$${last:-0.0.0}" | awk -F. -v b="$(BUMP)" '{ \
	    if (b == "major") print ($$1 + 1) ".0.0"; \
	    else if (b == "minor") print $$1 "." ($$2 + 1) ".0"; \
	    else print $$1 "." $$2 "." ($$3 + 1) }'); \
	  v=$$(printf '%s\n' "$$v" "$$floor" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1); \
	fi; \
	echo "$$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$$' || { echo "not a version like 1.2.3: '$$v'" >&2; exit 1; }; \
	if docker buildx imagetools inspect "$(IMAGE):$$v" >/dev/null 2>&1; then echo "$(IMAGE):$$v already exists" >&2; exit 1; fi; \
	echo "$$v"

.PHONY: push-server server-version
push-server:
	@set -e; v=$$($(next_version)); ctx=.; \
	if [ -n "$(REF)" ]; then \
	  ctx=$$(mktemp -d); trap 'rm -rf "$$ctx"' EXIT; \
	  git archive "$(REF)" | tar -x -C "$$ctx"; \
	  cp .dockerignore "$$ctx"/ 2>/dev/null || true; \
	fi; \
	c=$$(git rev-parse --short=10 $(or $(REF),HEAD)); \
	if [ -z "$(REF)" ] && [ -n "$$(git status --porcelain --untracked-files=no)" ]; then c="$$c-dirty"; fi; \
	docker buildx build --platform $(PLATFORMS) -f apps/lightcraft-server/Dockerfile \
	  --build-arg "LIGHTCRAFT_VERSION=$$v" --build-arg "LIGHTCRAFT_COMMIT=$$c" \
	  --label "org.opencontainers.image.version=$$v" -t "$(IMAGE):$$v" -t "$(IMAGE):latest" --push "$$ctx"; \
	echo "pushed $(IMAGE):$$v (also :latest)"
	@docker buildx imagetools inspect $(IMAGE):latest | grep -E 'Name:|Digest:|Platform:' | head -4

server-version:
	@$(next_version)

# Build LightCraft.app (ad-hoc signed unless MACOS_SIGN_IDENTITY is set) and its DMG.
#
#   make macos-app                   # this Mac's architecture -> target/macos-package/LightCraft.app
#   make macos-app ARCH=universal    # or x86_64; set CRAFT_FONTS_DIR=../craft-fonts for CJK glyphs
ARCH ?= $(shell uname -m | sed 's/arm64/aarch64/')

.PHONY: macos-app
macos-app:
	packaging/macos/package.sh --arch $(ARCH)
