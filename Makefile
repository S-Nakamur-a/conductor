.PHONY: install build dev clean

install:
	cargo install --path .

build:
	cargo build --release

dev:
	cargo run

clean:
	cargo clean
