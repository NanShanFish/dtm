.PHONY: build test-inter

CONTAINER_ENGINE ?= podman
IMAGE ?= dtm-test-inter:local

build:
	cargo build --release

test-inter: build
	$(CONTAINER_ENGINE) build --file Dockerfile.test-inter --tag $(IMAGE) .
	$(CONTAINER_ENGINE) run --rm --interactive --tty --init $(IMAGE)
