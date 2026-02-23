.PHONY: install build dev clean

install:
	cargo install --path .
	cd mcp/conductor-comment && npm install

build:
	cargo build --release

dev:
	cargo run

clean:
	cargo clean
	rm -rf mcp/conductor-comment/dist
