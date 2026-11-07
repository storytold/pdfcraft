changed := "pdfcraft"

pr-check crates=changed: fmt (clippy crates) (test crates) gates

fmt:
    cargo fmt --all -- --check

clippy crates=changed:
    cargo clippy --locked --all-targets $(printf -- '-p %s ' {{ crates }}) -- -D warnings

test crates=changed:
    cargo test --locked $(printf -- '-p %s ' {{ crates }})

gates:
    cargo xtask layers
    cargo xtask assets

ci:
    cargo xtask ci
