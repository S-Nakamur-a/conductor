.PHONY: install build dev clean hooks fmt

install:
	cargo install --path .

build:
	cargo build --release

dev:
	cargo run

clean:
	cargo clean

fmt:
	cargo fmt --all

hooks:
	git config core.hooksPath .githooks
