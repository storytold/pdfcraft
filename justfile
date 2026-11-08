pr-check +crates: fmt (clippy crates) (test crates) gates

fmt:
    cargo fmt --all -- --check

clippy +crates:
    set -f; cargo clippy --locked --all-targets $(printf -- '-p %s ' {{ crates }}) -- -D warnings

test +crates:
    set -f; cargo test --locked $(printf -- '-p %s ' {{ crates }})

gates:
    cargo xtask layers
    cargo xtask assets

i18n-hash:
    nix hash convert --hash-algo sha256 --to sri "$(nix-prefetch-url --unpack https://github.com/storytold/craft-i18n/archive/$(cat craft-i18n.rev).tar.gz)"

ci:
    cargo xtask ci
