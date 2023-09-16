precommit:
	cargo fmt
	cargo check
	typos
	cargo test
	rustup default 1.67.0
	cargo check
	rustup default stable
