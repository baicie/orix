.PHONY: check fmt lint test doc security release publish dry-run yank

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

# Full release: check → publish crates → git tag → push.
#
# Flags (set as environment variables or make arguments):
#   orix_dry_run=1          preview only, no changes made
#   orix_crates_only=1      only publish crates, skip git tag and push
#   orix_skip_crates=1      skip crates.io publish, only git tag + push
#
# Custom xtask arguments:
#   ARGS="--version 0.2.0"   override version
#   ARGS="--force"           yank existing version before re-publishing
#
# Examples:
#   make release                       # full release
#   make release ARGS="--version 0.2.0"
#   make release orix_dry_run=1 ARGS="--version 0.2.0"
#   make release orix_crates_only=1
#   make release orix_skip_crates=1
release:
	@if [ "$(orix_skip_crates)" = "1" ]; then \
		cargo xtask release --skip-crates $(ARGS); \
	elif [ "$(orix_crates_only)" = "1" ]; then \
		@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then echo "CARGO_REGISTRY_TOKEN not set"; exit 1; fi; \
		cargo xtask release --crates-only $(ARGS); \
	elif [ "$(orix_dry_run)" = "1" ]; then \
		cargo xtask release --dry-run $(ARGS); \
	else \
		@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then echo "CARGO_REGISTRY_TOKEN not set"; exit 1; fi; \
		cargo xtask release $(ARGS); \
	fi

# Publish all crates to crates.io in topological order.
# Usage:
#   make publish                         # show publish plan
#   make publish orix_dry_run=0         # actually publish
#   make publish orix_dry_run=0 ARGS="--force --version 0.1.0"
publish:
	@if [ "$(orix_dry_run)" != "0" ]; then \
		cargo xtask publish-crates --dry-run $(ARGS); \
	else \
		@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then \
			echo "CARGO_REGISTRY_TOKEN not set"; exit 1; fi; \
		cargo xtask publish-crates $(ARGS); \
	fi

# Preview the full release flow (no changes made).
dry-run:
	cargo xtask release --dry-run $(ARGS)

# Yank specific version of crates from crates.io.
# Usage:
#   make yank VERSION=0.1.0                         # yank all crates
#   make yank VERSION=0.1.0 CRATES="orix-cli"       # yank specific crates
yank:
	@if [ -z "$$CARGO_REGISTRY_TOKEN" ]; then \
		echo "CARGO_REGISTRY_TOKEN not set"; exit 1; fi
	@if [ -z "$(VERSION)" ]; then \
		echo "VERSION is required, e.g. make yank VERSION=0.1.0"; exit 1; fi
	cargo xtask yank $(VERSION) $(foreach c,$(CRATES),--crates $(c))
