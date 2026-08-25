.PHONY: install build dev clean fmt

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
