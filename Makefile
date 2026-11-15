.PHONY: all help frontend backend test lint fmt check-secrets clean

all: frontend backend ## Build the frontend, then the release backend

help: ## List available targets
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-16s %s\n", $$1, $$2}'

frontend: ## Build the Svelte UI into frontend/dist (required before cargo)
	@echo "=== 1. Building Svelte Frontend ==="
	cd frontend && npm ci && npm run build

backend: frontend ## Build the release binary (embeds frontend/dist)
	@echo "=== 2. Building Rust Backend ==="
	cargo build --release

test: frontend ## Run unit and integration tests
	@echo "=== 3. Running Unit and Integration Tests ==="
	cargo test --workspace

lint: frontend ## Check formatting and run clippy with warnings denied
	@echo "=== Lint: rustfmt + clippy ==="
	cargo fmt --all -- --check
	cargo clippy --all-targets --locked -- -D warnings

fmt: ## Format Rust sources
	cargo fmt --all

check-secrets: ## Run gitleaks secret and RFC1918 audit
	@echo "=== 4. Running Security Leak Audit ==="
	gitleaks detect --source . -v

clean: ## Remove build artifacts
	rm -rf target frontend/dist frontend/node_modules
