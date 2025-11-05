alias c := check
alias t := test
alias l := lint
alias au := audit
alias a := all

test:
	cargo hack --workspace test

lint:
	cargo hack --workspace clippy -- --deny warnings

check-feature-powerset:
	cargo hack --workspace check --feature-powerset --no-dev-deps

check-each-feature:
	cargo hack --workspace check --each-feature --no-dev-deps

check: check-each-feature check-feature-powerset

check-advisories:
	cargo deny --workspace check advisories

check-bans:
	cargo deny --workspace check bans

check-licenses:
	cargo deny --workspace check licenses

check-sources:
	cargo deny --workspace  check sources

check-outdated:
	cargo outdated --workspace --exit-code 1

audit: check-advisories check-outdated check-bans check-licenses check-sources

view-supply-chain:
	cargo supply-chain crates

all: check test lint audit

# vim: set ft=make :
