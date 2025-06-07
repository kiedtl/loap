deploy:
	cargo build --release
	ssh illuvatar -- 'sudo systemctl stop loap'
	scp target/release/loap illuvatar:~kiedtl/loap/
	scp -r public illuvatar:~kiedtl/loap/
	scp -r content illuvatar:~kiedtl/loap/
	ssh illuvatar -- 'sudo systemctl start loap'
