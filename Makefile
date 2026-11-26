.PHONY: help build test goldens fmt fmt-check clippy deny structure readme-links standards-check ci-fast ci-lint ci install update-plugins clean

CARGO ?= cargo
# Every build uses the committed Cargo.lock as is; a stale lock fails instead
# of being rewritten (STD-05 R23).
LOCKED := --locked
BINARY := scribe
INSTALL_BIN_DIR ?= $(HOME)/.cargo/bin
# The plugin as both local marketplace manifests name it.
PLUGIN_ID := scribe@scribe-local
# `make goldens UPDATE=1` rewrites the golden files instead of comparing.
UPDATE ?=
# `STANDARDS_STRICT=1` makes a missing docs/standards/ a failure (CI sets it).
STANDARDS_STRICT ?=
# GitHub Actions sets CI=true. There a missing gate tool fails the gate;
# locally it warns and skips or falls back (STD-04 R10, decision in
# docs/design/scribe/4_decisions.md).
CI ?=

# $(call missing_tool,<tool>,<what it provides>): fail under CI, else warn.
define missing_tool
	if [ "$(CI)" = "true" ]; then \
		echo "error: $(1) is not installed; CI requires it for $(2)." >&2; exit 1; \
	fi; \
	echo "warning: $(1) not installed; continuing without $(2) (CI requires it)." >&2; \
	echo "         install: cargo install $(1) --locked" >&2
endef

help:
	@echo "scribe make targets"
	@echo ""
	@echo "  make ci-fast          Pre-handoff gate, no compile: fmt, structure, readme-links, standards-check"
	@echo "  make ci-lint          clippy -D warnings + cargo-deny (skipped locally if not installed)"
	@echo "  make test             All tests (nextest when installed, else cargo test) + doctests"
	@echo "  make goldens          Compare --help and output goldens with the built binary"
	@echo "  make goldens UPDATE=1 Regenerate goldens after an intended surface change"
	@echo "  make standards-check  Run the vendored standards check (docs/standards/check.sh)"
	@echo "  make ci               Everything above; what CI runs"
	@echo "  make update-plugins   Refresh the installed Claude Code and Codex plugins from this checkout"
	@echo "  make build | fmt | install | clean"

build:
	$(CARGO) build --workspace $(LOCKED)

# ---------------------------------------------------------------------------
# Gates
# ---------------------------------------------------------------------------

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

# Source-only structure checks: the architecture map (STD-02 R43), dependency
# direction (STD-02 R1-R6), unit-test module reachability (STD-02 R19),
# terminal/stream ownership (STD-01 R17), current-format claims in the store
# spec against FORMAT, and the cases that test those gates themselves
# (STD-04 R2, R12).
structure:
	./scripts/check-architecture-map.sh
	python3 scripts/sync-skills.py --check
	python3 scripts/sync-orbit-packages.py --check
	./scripts/check-dependency-direction.sh
	./scripts/check-test-modules.sh
	./scripts/check-terminal-guard.sh
	python3 scripts/check-store-format-claims.py
	python3 scripts/check-session-id-outcomes.py
	./scripts/tests/check-gates.sh

# STD-09 R12: every relative link, heading anchor and image path in README.md
# resolves, in Markdown and in HTML href, src and srcset. External URLs are not
# checked: the gate needs no network.
readme-links:
	python3 scripts/check-readme-links.py

clippy:
	$(CARGO) clippy --workspace --all-targets $(LOCKED) -- -D warnings

# Supply chain (STD-02 R23, STD-05 R23/R24): advisories, licenses, sources.
deny:
	@if $(CARGO) deny --version >/dev/null 2>&1; then \
		cargo_home="$${CARGO_HOME:-$$HOME/.cargo}"; \
		deny_db="$$cargo_home/advisory-dbs"; \
		writable_parent="$$deny_db"; \
		while [ ! -d "$$writable_parent" ]; do \
			parent=$$(dirname "$$writable_parent"); \
			[ "$$parent" = "$$writable_parent" ] && break; \
			writable_parent="$$parent"; \
		done; \
		if [ ! -w "$$writable_parent" ] || [ ! -x "$$writable_parent" ]; then \
			mkdir -p "$(CURDIR)/target/cargo-deny"; \
			deny_config="$(CURDIR)/target/cargo-deny/deny-$$$$.toml"; \
			sed 's@^db-path = "\$$CARGO_HOME/advisory-dbs"$$@db-path = "$(CURDIR)/target/cargo-deny/advisory-dbs"@' deny.toml > "$$deny_config"; \
			$(CARGO) deny check --config "$$deny_config"; \
		else \
			$(CARGO) deny check; \
		fi; \
	else \
		$(call missing_tool,cargo-deny,the supply-chain check); \
	fi

# nextest enforces per-test timeouts (.config/nextest.toml, STD-03 R21/R22)
# and fails a selection that matches no test (STD-04 R10); plain cargo test
# does neither, so the fallback says so. nextest does not run doctests, so
# they are a second leg; the core crate carries at least one (STD-04 R14).
test:
	@if $(CARGO) nextest --version >/dev/null 2>&1; then \
		$(CARGO) nextest run --workspace --all-targets $(LOCKED) --no-tests=fail; \
	else \
		$(call missing_tool,cargo-nextest,per-test timeouts and the zero-test check); \
		$(CARGO) test --workspace --all-targets $(LOCKED); \
	fi
	$(CARGO) test --workspace --doc $(LOCKED)

# Goldens of the shipped surface (STD-01 R24): captured from the built binary.
goldens:
	@if $(CARGO) nextest --version >/dev/null 2>&1; then \
		UPDATE_GOLDENS=$(if $(UPDATE),1,0) $(CARGO) nextest run -p $(BINARY) --test goldens $(LOCKED) --no-tests=fail; \
	else \
		$(call missing_tool,cargo-nextest,the zero-test check); \
		UPDATE_GOLDENS=$(if $(UPDATE),1,0) $(CARGO) test -p $(BINARY) --test goldens $(LOCKED); \
	fi

# The constellation standards this repo adopted are vendored under
# docs/standards/ by operations/scripts/sync-standards.sh, which also writes
# check.sh. Until then this warns and passes, so a fresh copy of the template
# builds; with STANDARDS_STRICT=1 (set in CI) a missing vendor fails.
standards-check:
	@if [ -f docs/standards/check.sh ]; then \
		sh docs/standards/check.sh; \
	elif [ "$(STANDARDS_STRICT)" = "1" ]; then \
		echo "error: standards not vendored yet (no docs/standards/check.sh)." >&2; \
		echo "       from the constellation root: operations/scripts/sync-standards.sh --adopt STD-01,STD-02,STD-03,STD-04,STD-05 <this repo>" >&2; \
		exit 1; \
	else \
		echo "warning: standards not vendored yet (no docs/standards/check.sh); skipping." >&2; \
		echo "         from the constellation root: operations/scripts/sync-standards.sh --adopt STD-01,STD-02,STD-03,STD-04,STD-05 <this repo>" >&2; \
	fi

# Pre-handoff gate for agents: no compile, and every parity check CI runs
# that needs no build (STD-04 R12). Goldens need the binary, so they are
# their own target.
ci-fast: fmt-check structure readme-links standards-check

ci-lint: clippy deny

# Full pass. Keep aligned with .github/workflows/ci.yml.
ci: ci-fast ci-lint test goldens

# ---------------------------------------------------------------------------
# Install
# ---------------------------------------------------------------------------

install:
	$(CARGO) build --release --bin $(BINARY) $(LOCKED)
	install -d $(INSTALL_BIN_DIR)
	install -m 755 target/release/$(BINARY) $(INSTALL_BIN_DIR)/$(BINARY)

# Refresh the plugin each harness installed from this checkout's `scribe-local`
# marketplace (docs/guide.md). A harness that is not on PATH is skipped; a
# failing command stops the target. Restart the harness afterwards.
update-plugins:
	@if command -v claude >/dev/null 2>&1; then \
		set -ex; \
		claude plugin marketplace update scribe-local; \
		claude plugin update $(PLUGIN_ID); \
	else \
		echo "update-plugins: claude is not on PATH; skipped." >&2; \
	fi
	@if command -v codex >/dev/null 2>&1; then \
		set -ex; \
		codex plugin add $(PLUGIN_ID) --json; \
	else \
		echo "update-plugins: codex is not on PATH; skipped." >&2; \
	fi

clean:
	$(CARGO) clean
