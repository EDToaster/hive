.PHONY: ci check fmt clippy test coverage build

ci: check fmt clippy test coverage build

check:
	cargo check --all-targets

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test --all-targets

coverage:
	cargo llvm-cov --all-targets --fail-under-lines 50

build:
	cargo build --release
