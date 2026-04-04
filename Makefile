# EngineMev — Build, Lint, Test, Coverage

.PHONY: check build test lint coverage coverage-html clean

# Quick compilation check
check:
	cargo check

# Release build
build:
	cargo build --release

# Run all unit tests
test:
	cargo test --test unit -- --nocapture

# Run e2e tests (requires RPC_URL)
test-e2e:
	cargo test --features e2e --test e2e -- --nocapture

# Run Surfpool e2e tests (requires RPC_URL + surfpool)
test-surfpool:
	cargo test --features e2e_surfpool --test e2e_surfpool -- --nocapture

# Lint with clippy (warnings = errors in CI)
lint:
	cargo clippy --all-targets -- -D warnings

# Lint (warnings only, no failure)
lint-warn:
	cargo clippy --all-targets

# Test coverage report (text summary)
coverage:
	cargo tarpaulin --test unit --out stdout --skip-clean

# Test coverage report (HTML)
coverage-html:
	cargo tarpaulin --test unit --out html --output-dir coverage/ --skip-clean
	@echo "Coverage report: coverage/tarpaulin-report.html"

# Full CI check: lint + test + coverage
ci: lint test coverage

clean:
	cargo clean
