.PHONY: all frontend backend test clean check-secrets

all: frontend backend

frontend:
	@echo "=== 1. Building Svelte Frontend ==="
	cd frontend && npm ci && npm run build

backend:
	@echo "=== 2. Building Rust Backend ==="
	cargo build --release

test: frontend
	@echo "=== 3. Running Unit and Integration Tests ==="
	cargo test --workspace

check-secrets:
	@echo "=== 4. Running Security Leak Audit ==="
	gitleaks detect --source . -v

clean:
	rm -rf target frontend/dist frontend/node_modules