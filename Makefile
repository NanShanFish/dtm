.PHONY: build build-test-inter test-inter

CONTAINER_ENGINE ?= podman
IMAGE ?= dtm-test-inter:local

build:
	cargo build --release

build-test-inter:
	cargo rustc --release --target-dir target/test-inter -- -C target-feature=+crt-static

test-inter: build-test-inter
	$(CONTAINER_ENGINE) build --file Dockerfile.test-inter --tag $(IMAGE) .
	$(CONTAINER_ENGINE) run --rm --interactive --tty --init $(IMAGE)
