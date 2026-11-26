# smart-vault (pure Rust #![no_std] + ASM + CSR) developer targets — made with <3 from 8b.is-crew

.PHONY: help build release check-nostd sign test lint fmt bench validate contracts mcp

help:
	@echo "smart-vault developer targets:"
	@echo "  make build        Compile debug workspace"
	@echo "  make release      Compile LTO-optimized native release binary"
	@echo "  make check-nostd  Verify smart-vault-core compiles with --no-default-features"
	@echo "  make sign         Build release and apply macOS codesign + SHA-256 verification"
	@echo "  make test         Run all Rust unit & integration tests + contracts"
	@echo "  make lint         Run workspace clippy with strict dead-code & warning gates"
	@echo "  make fmt          Check Rust formatting with cargo fmt --all --check"
	@echo "  make bench        Build & run Criterion benchmarks"
	@echo "  make validate     Run contract & package validators"
	@echo "  make mcp          Start stdio MCP server"

build:
	cargo build --workspace

release:
	cargo build --release

check-nostd:
	cargo check -p smart-vault-core --no-default-features

sign: release
	cargo run --release --quiet -- sign "$$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1\/release\/smart-vault/p')" --write-sha256

test:
	cargo test --workspace
	cargo run --quiet -- contracts --repo-root .
	cargo run --quiet -- package validate --repo-root .

lint:
	cargo clippy --workspace --all-targets -- -D warnings -D dead_code -D unused_imports -D clippy::redundant_clone -D clippy::needless_collect -D clippy::map_unwrap_or

fmt:
	cargo fmt --all -- --check

bench:
	cargo bench

validate:
	cargo run --quiet -- contracts --repo-root .
	cargo run --quiet -- package validate --repo-root .

mcp:
	cargo run --quiet -- mcp
