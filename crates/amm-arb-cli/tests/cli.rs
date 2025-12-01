//! Runs the `amm-arb` binary against the committed fixtures.

use std::process::Command;

fn run(args: &[&str]) -> String {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let out = Command::new(env!("CARGO_BIN_EXE_amm-arb"))
        .args(args)
        .current_dir(root)
        .output()
        .expect("binary runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn scan_finds_the_planted_cycle() {
    let out = run(&["scan", "--fixtures", "fixtures/planted.json"]);
    assert!(out.contains("negative cycle found"), "{out}");
    assert!(out.contains("profitable after gas"), "{out}");
}

#[test]
fn scan_json_is_machine_readable() {
    let out = run(&["scan", "--fixtures", "fixtures/planted.json", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let best = &v["opportunities"][0];
    assert_eq!(best["hops"][0]["token_in"], best["start_token"]);
}

#[test]
fn mainnet_snapshot_has_no_arbitrage() {
    let out = run(&["scan", "--fixtures", "fixtures/mainnet-23700000.json"]);
    assert!(out.contains("no negative cycle"), "{out}");
}

#[test]
fn quote_on_mainnet_snapshot() {
    // Matches Uniswap's QuoterV2 at block 23700000 (checked by tests/mainnet_quoter.rs).
    let out = run(&[
        "quote",
        "--fixtures",
        "fixtures/mainnet-23700000.json",
        "--pool",
        "0x88e6A0c2dDD26FEEb64F039a2c41296FcB3f5640",
        "--token-in",
        "WETH",
        "--amount",
        "1000",
    ]);
    assert!(out.contains("raw 3618298009284"), "{out}");
}
