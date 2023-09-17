precommit:
	rustup default stable
	cargo fmt
	cargo check
	typos
	cargo test
	rustup default 1.70.0
	cargo check
	rustup default stable
