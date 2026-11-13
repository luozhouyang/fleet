PCARGO ?= /home/luozhouyang/Applications/pcargo/cargo
CARGO = $(PCARGO)
PNPM ?= pnpm

.DEFAULT_GOAL := help

help:
	@echo "build test fmt lint dev-control-plane dev-daemon dev-web web-install web-build e2e e2e-api e2e-ui"

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --workspace --release

test: test-rust test-web

test-rust:
	$(CARGO) test --workspace

test-web:
	$(PNPM) -r --if-present test

fmt:
	$(CARGO) fmt --all
	$(PNPM) -r --if-present format

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(PNPM) -r --if-present lint

dev-control-plane:
	$(CARGO) run -p agent-fleet-control-plane -- serve

dev-daemon:
	$(CARGO) run -p agent-fleet-daemon -- serve

dev-web:
	$(PNPM) --filter @agent-fleet/web dev

web-install:
	$(PNPM) install

web-build:
	$(PNPM) --filter @agent-fleet/web build

e2e:
	$(CARGO) test --workspace --test e2e -- --include-ignored

e2e-api:
	./scripts/test/e2e_smoke.sh

e2e-ui:
	./scripts/test/ui_e2e.sh
