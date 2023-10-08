precommit:
	rustup default stable
	cargo fmt
	cargo check
	typos
	cargo test
	rustup default 1.70.0
	cargo check
	rustup default stable

run-dev:
	cargo run -- --url=https://example.com \
		--mint=https://8333.space:3338 \
		--invoice-description=test \
		--nsec=60f614d09bcd6dd712cb505528204f375f30d449ba87e186ed7b6baa0f802d07 \
		--db-path='./cashu_lnurl.redb' \
		--proxy=true \
		--fee=1 \
		--cln-path="/home/thesimplekid/.lightning/signet/lightning-rpc" \
		--zapper=true \
		--pay-index-path='./pay_index' \
		--address='127.0.0.1' \
		--port=8080 \
		--relays=wss://relay.damus.io \
		--two-char-price=5 \
		--three-char-price=4 \
		--four-char-price=3 \
		--other-char-price=2 \
