set shell := ["sh", "-cu"]

default:
    @just --list

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace --all-targets

doctest:
    cargo test --workspace --doc

build:
    cargo build --workspace --locked

build-release:
    cargo build --workspace --release --locked

run *ARGS:
    cargo run -p sourcefour -- {{ARGS}}

check: fmt-check lint test doctest build build-release
