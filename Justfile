deploy:
	cargo leptos build --release
	scp target/release/loap illuvatar:~kiedtl/loap/
	scp -r target/site illuvatar:~kiedtl/loap/
