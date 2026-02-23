.PHONY: install build dev clean

install:
	cargo install --path .
	cd plugins/conductor/mcp/conductor-comment && npm install

build:
	cargo build --release

dev:
	cargo run

clean:
	cargo clean
	rm -rf plugins/conductor/mcp/conductor-comment/dist
