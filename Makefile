.PHONY: check fmt lint test doc security release publish dry-run

check:
	cargo xtask check

fmt:
	cargo xtask fmt

lint:
	cargo xtask lint

test:
	cargo xtask test

doc:
	cargo xtask doc

security:
	cargo xtask security

# Full release: check → publish crates → git tag → push to origin.
# Use dry-run to preview without making changes.
# Use SKIP_CRATES=1 to skip crates.io publish.
# Use CRATES_ONLY=1 to skip git tag (only publish crates).
release:
	@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then echo "CARGO_REGISTRY_TOKEN not set"; exit 1; fi
	@if [ "$(orix_dry_run)" = "1" ]; then \
		cargo xtask release --dry-run; \
	elif [ "$(orix_crates_only)" = "1" ]; then \
		cargo xtask release --crates-only; \
	else \
		cargo xtask release; \
	fi

# Publish all crates to crates.io in topological order.
# Default: show plan (dry-run). Set orix_dry_run=0 to actually publish.
publish:
	@if [ "$(orix_dry_run)" != "0" ]; then \
		cargo xtask publish-crates --dry-run; \
	else \
		@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then \
			echo "CARGO_REGISTRY_TOKEN not set"; exit 1; \
		fi; \
		cargo xtask publish-crates; \
	fi

# Quick dry-run of the full release flow (no changes made).
dry-run:
	cargo xtask release --dry-run
