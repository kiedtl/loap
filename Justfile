deploy:
	cargo build --release
	scp target/release/loap illuvatar:~kiedtl/loap/
	scp -r public illuvatar:~kiedtl/loap/
	scp -r content illuvatar:~kiedtl/loap/
