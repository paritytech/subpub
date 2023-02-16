check-style:
  cargo fmt --all -- --check

lint:
  cargo clippy --all-targets --workspace -- -Dwarnings

typecheck:
  cargo check --all-targets --workspace

check-installation:
  cargo install --quiet --path . --root target

all-checks: check-style lint typecheck check-installation
