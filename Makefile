# Build the sync server image and push it to the registry (docs/self-hosting.md).
#
#   make push-server                 # amd64 + arm64, tag latest, from the working tree
#   make push-server REF=HEAD        # from the committed code only (ignores uncommitted work)
#   make push-server TAG=v0.2.1 PLATFORMS=linux/amd64
#
# Needs `docker login registry.eliasruiz.com` once, and a buildx that can push multi-platform
# (Docker Desktop / OrbStack with the containerd image store).

IMAGE     ?= registry.eliasruiz.com/library/lightcraft
TAG       ?= latest
PLATFORMS ?= linux/amd64,linux/arm64
REF       ?=

.PHONY: push-server
push-server:
	@set -e; ctx=.; \
	if [ -n "$(REF)" ]; then \
	  ctx=$$(mktemp -d); trap 'rm -rf "$$ctx"' EXIT; \
	  git archive "$(REF)" | tar -x -C "$$ctx"; \
	  cp .dockerignore "$$ctx"/ 2>/dev/null || true; \
	fi; \
	docker buildx build --platform $(PLATFORMS) -f apps/lightcraft-server/Dockerfile \
	  -t $(IMAGE):$(TAG) --push "$$ctx"
	@docker buildx imagetools inspect $(IMAGE):$(TAG) | grep -E 'Name:|Digest:|Platform:' | head -4
