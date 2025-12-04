# Same commands CI runs. `make check` before pushing.

.PHONY: check rust contracts vectors vendor bench fork

check: rust contracts

rust:
	cargo fmt --all --check
	cargo clippy --all-targets -- -D warnings
	cargo test

contracts:
	cd contracts && forge fmt --check && forge build && forge test

# Regenerate the differential vectors from the vendored Uniswap bytecode.
vectors:
	cd contracts && forge script script/GenerateVectors.s.sol

# Rebuild vendored Uniswap bytecode from the submodules (needs Node).
vendor:
	cd contracts/vendor && npm ci && npm run build

bench:
	cargo bench -p amm-arb --bench quotes

# Needs an archive endpoint, e.g. MAINNET_RPC_URL=https://eth.drpc.org make fork
fork:
	cd contracts && forge test --match-contract Fork -vv
	cargo test -p amm-arb-cli --test mainnet_quoter -- --nocapture
