.PHONY: install build dev clean fmt wt-stamp wt-reset

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

wt-stamp:
	python3 scripts/wt-version.py stamp

wt-reset:
	python3 scripts/wt-version.py reset
