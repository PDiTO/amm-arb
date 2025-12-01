//! `amm-arb`: scan Uniswap V2/V3 pools for cyclic arbitrage and quote swaps
//! with exact integer math, from a JSON fixture or live over RPC.

use amm_arb_cli::rpc;

use std::{fmt::Write as _, path::PathBuf, time::Instant};

use alloy::primitives::{Address, I256, U256};
use amm_arb::{
    fixture::Fixture,
    market::{Market, PoolState},
    scan::{Opportunity, ScanConfig, ScanReport, scan},
};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
#[command(name = "amm-arb", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find, size and rank cyclic arbitrage.
    Scan {
        #[command(flatten)]
        source: Source,
        /// Longest cycle to consider.
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(2..=4))]
        max_hops: u8,
        /// Gas price in gwei used to cost each cycle.
        #[arg(long, default_value_t = 10.0)]
        gas_price_gwei: f64,
        /// How many opportunities to print.
        #[arg(long, default_value_t = 10)]
        top: usize,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Quote an exact-input swap on one pool.
    Quote {
        #[command(flatten)]
        source: Source,
        /// Pool address.
        #[arg(long)]
        pool: Address,
        /// Input token, by symbol or address.
        #[arg(long)]
        token_in: String,
        /// Input amount in whole tokens, e.g. `1.5`.
        #[arg(long)]
        amount: String,
    },
    /// Load pools over RPC and write them to a fixture file.
    Snapshot {
        #[command(flatten)]
        rpc: RpcArgs,
        /// Output path.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Args)]
struct Source {
    /// Read pool state from a fixture file.
    #[arg(long, conflicts_with = "rpc")]
    fixtures: Option<PathBuf>,
    #[command(flatten)]
    rpc: RpcArgs,
}

#[derive(Args)]
struct RpcArgs {
    /// JSON-RPC endpoint to load live state from.
    #[arg(long)]
    rpc: Option<String>,
    /// JSON list of `{ "kind": "v2" | "v3", "address": "0x..." }`. Defaults to a
    /// built-in set of 17 deep mainnet pools.
    #[arg(long, requires = "rpc")]
    pools: Option<PathBuf>,
    /// Block to read at. Defaults to the latest block.
    #[arg(long, requires = "rpc")]
    block: Option<u64>,
    /// Ticks of V3 range to load on each side of the current tick (10,000
    /// ticks is a factor of ~2.7 in price). Swaps that would leave the loaded
    /// range fail instead of returning a wrong quote.
    #[arg(long, default_value_t = 10_000, requires = "rpc")]
    tick_range: i32,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Scan {
            source,
            max_hops,
            gas_price_gwei,
            top,
            json,
        } => {
            let (market, origin) = load(&source).await?;
            let config = ScanConfig {
                max_hops: max_hops.into(),
                gas_price_wei: (gas_price_gwei * 1e9) as u128,
                ..ScanConfig::default()
            };
            let report = scan(&market, &config);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report_json(&market, &report))?
                );
            } else {
                print_report(&market, &report, &origin, top, gas_price_gwei);
            }
        }
        Command::Quote {
            source,
            pool,
            token_in,
            amount,
        } => {
            let (market, _) = load(&source).await?;
            quote(&market, pool, &token_in, &amount)?;
        }
        Command::Snapshot { rpc, out } => {
            let url = rpc
                .rpc
                .as_deref()
                .ok_or_else(|| anyhow!("--rpc is required"))?;
            let (market, loader) = load_rpc(url, &rpc).await?;
            let mut fixture = Fixture::from_market(&market);
            fixture.description = Some(format!(
                "{} pools read at block {} via Multicall3 by `amm-arb snapshot`",
                market.pools.len(),
                loader.block
            ));
            fixture.chain_id = Some(loader.chain_id);
            fixture.block = Some(loader.block);
            fixture.save(&out)?;
            eprintln!(
                "wrote {} pools at block {} to {}",
                market.pools.len(),
                loader.block,
                out.display()
            );
        }
    }
    Ok(())
}

async fn load(source: &Source) -> Result<(Market, String)> {
    if let Some(path) = &source.fixtures {
        let fixture = Fixture::load(path).with_context(|| format!("reading {}", path.display()))?;
        let origin = match fixture.block {
            Some(b) => format!("{} (block {b})", path.display()),
            None => path.display().to_string(),
        };
        return Ok((fixture.to_market()?, origin));
    }
    let Some(url) = source.rpc.rpc.as_deref() else {
        bail!("pass --fixtures <file> or --rpc <url>");
    };
    let t = Instant::now();
    let (market, loader) = load_rpc(url, &source.rpc).await?;
    Ok((
        market,
        format!(
            "RPC at block {} (loaded in {:.1?})",
            loader.block,
            t.elapsed()
        ),
    ))
}

async fn load_rpc(url: &str, args: &RpcArgs) -> Result<(Market, rpc::Loader)> {
    let refs = match &args.pools {
        Some(path) => serde_json::from_str(&std::fs::read_to_string(path)?)?,
        None => rpc::default_pools(),
    };
    let loader = rpc::Loader::connect(url, args.block, args.tick_range).await?;
    let market = loader.load(&refs).await?;
    Ok((market, loader))
}

fn print_report(market: &Market, report: &ScanReport, origin: &str, top: usize, gwei: f64) {
    println!("source      {origin}");
    println!(
        "graph       {} pools, {} tokens, {} directed edges",
        market.pools.len(),
        report.tokens,
        report.edges
    );
    println!(
        "search      Bellman-Ford: {}; {} negative cycles of <= 3 hops; {:.1?}",
        if report.any_negative_cycle {
            "negative cycle found"
        } else {
            "no negative cycle"
        },
        report.candidates,
        report.search_time
    );
    let evals: u32 = report.opportunities.iter().map(|o| o.evaluations).sum();
    println!(
        "sizing      {} cycles sized with {} exact quotes of the full cycle; {:.1?}",
        report.opportunities.len(),
        evals,
        report.sizing_time
    );
    println!("gas         {gwei} gwei");
    if report.opportunities.is_empty() {
        println!("\nNo cycle is profitable before gas at this block.");
        return;
    }
    let profitable = report
        .opportunities
        .iter()
        .filter(|o| o.net_profit_wei.is_positive())
        .count();
    println!(
        "\n{} of {} cycles are profitable after gas. Top {}:",
        profitable,
        report.opportunities.len(),
        top.min(report.opportunities.len())
    );
    let gas = market.token(market.gas_token).map_or(18, |t| t.decimals);
    let gas_symbol = market.symbol(market.gas_token);
    for (i, o) in report.opportunities.iter().take(top).enumerate() {
        println!(
            "\n#{:<2} net {} {gas_symbol}   gross {} {}   gas {}k = {} {gas_symbol}",
            i + 1,
            fmt_signed(o.net_profit_wei, gas),
            fmt_units(o.gross_profit, decimals(market, o.start_token)),
            market.symbol(o.start_token),
            o.gas_units / 1000,
            fmt_units(o.gas_cost_wei, gas),
        );
        print_path(market, o);
    }
}

fn print_path(market: &Market, o: &Opportunity) {
    let mut line = format!(
        "    {} {}",
        fmt_units(o.amount_in, decimals(market, o.start_token)),
        market.symbol(o.start_token)
    );
    for hop in &o.hops {
        let pool = market.pool(hop.pool).expect("pool in market");
        let _ = write!(
            line,
            "\n      -> [{} {:#}] {} {}",
            pool.label(),
            hop.pool,
            fmt_units(hop.amount_out, decimals(market, hop.token_out)),
            market.symbol(hop.token_out)
        );
    }
    println!("{line}");
}

fn report_json(market: &Market, report: &ScanReport) -> serde_json::Value {
    json!({
        "tokens": report.tokens,
        "edges": report.edges,
        "any_negative_cycle": report.any_negative_cycle,
        "candidates": report.candidates,
        "search_time_us": report.search_time.as_micros(),
        "sizing_time_us": report.sizing_time.as_micros(),
        "opportunities": report.opportunities.iter().map(|o| json!({
            "start_token": o.start_token,
            "start_symbol": market.symbol(o.start_token),
            "amount_in": o.amount_in.to_string(),
            "amount_out": o.amount_out.to_string(),
            "gross_profit": o.gross_profit.to_string(),
            "gas_units": o.gas_units,
            "gas_cost_wei": o.gas_cost_wei.to_string(),
            "net_profit_wei": o.net_profit_wei.to_string(),
            "hops": o.hops.iter().map(|h| json!({
                "pool": h.pool,
                "token_in": h.token_in,
                "token_out": h.token_out,
                "amount_in": h.amount_in.to_string(),
                "amount_out": h.amount_out.to_string(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn quote(market: &Market, pool: Address, token_in: &str, amount: &str) -> Result<()> {
    let pool = market
        .pool(pool)
        .ok_or_else(|| anyhow!("pool {pool} not in market"))?;
    let token_in = resolve_token(market, token_in)?;
    if token_in != pool.token0 && token_in != pool.token1 {
        bail!("{} is not traded by this pool", market.symbol(token_in));
    }
    let token_out = pool.other(token_in);
    let amount_in = parse_units(amount, decimals(market, token_in))?;
    let zero_for_one = pool.zero_for_one(token_in);

    let t = Instant::now();
    let out = pool.quote(token_in, amount_in);
    let elapsed = t.elapsed();
    let out = out.map_err(|e| anyhow!("pool would revert: {e}"))?;

    println!(
        "pool        {} {}/{} {}",
        pool.label(),
        market.symbol(pool.token0),
        market.symbol(pool.token1),
        pool.address
    );
    println!(
        "in          {} {}",
        fmt_units(amount_in, decimals(market, token_in)),
        market.symbol(token_in)
    );
    println!(
        "out         {} {}  (raw {out})",
        fmt_units(out, decimals(market, token_out)),
        market.symbol(token_out)
    );
    if let PoolState::V3(v3) = &pool.state {
        let r = v3.swap(zero_for_one, I256::try_from(amount_in)?, None)?;
        println!(
            "ticks       {} -> {}, crossed {} initialized ticks",
            v3.tick, r.tick, r.ticks_crossed
        );
        println!("sqrtPrice   {} -> {}", v3.sqrt_price_x96, r.sqrt_price_x96);
    }
    println!("quote time  {elapsed:.1?}");
    Ok(())
}

fn resolve_token(market: &Market, s: &str) -> Result<Address> {
    if let Ok(a) = s.parse::<Address>() {
        return Ok(a);
    }
    market
        .tokens
        .iter()
        .find(|t| t.symbol.eq_ignore_ascii_case(s))
        .map(|t| t.address)
        .ok_or_else(|| anyhow!("unknown token {s}"))
}

fn decimals(market: &Market, token: Address) -> u8 {
    market.token(token).map_or(18, |t| t.decimals)
}

/// Parses a decimal amount like `1.5` into raw units.
fn parse_units(s: &str, decimals: u8) -> Result<U256> {
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if frac.len() > decimals as usize {
        bail!("{s} has more than {decimals} decimal places");
    }
    let digits = format!("{whole}{frac:0<width$}", width = decimals as usize);
    U256::from_str_radix(digits.trim_start_matches('0').max("0"), 10)
        .map_err(|e| anyhow!("invalid amount {s}: {e}"))
}

/// Formats raw units with up to six significant decimals.
fn fmt_units(x: U256, decimals: u8) -> String {
    let s = x.to_string();
    let d = decimals as usize;
    let (whole, frac) = if s.len() > d {
        (s[..s.len() - d].to_owned(), s[s.len() - d..].to_owned())
    } else {
        ("0".to_owned(), format!("{s:0>d$}"))
    };
    let whole = group_thousands(&whole);
    let frac = frac.get(..6.min(d)).unwrap_or("").trim_end_matches('0');
    if frac.is_empty() {
        whole
    } else {
        format!("{whole}.{frac}")
    }
}

fn fmt_signed(x: I256, decimals: u8) -> String {
    let sign = if x.is_negative() { "-" } else { "+" };
    format!("{sign}{}", fmt_units(x.unsigned_abs(), decimals))
}

fn group_thousands(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_round_trip() {
        assert_eq!(parse_units("1.5", 6).unwrap(), U256::from(1_500_000));
        assert_eq!(parse_units("0", 18).unwrap(), U256::ZERO);
        assert!(parse_units("1.1234567", 6).is_err());
        assert_eq!(fmt_units(U256::from(1_500_000), 6), "1.5");
        assert_eq!(
            fmt_units(U256::from(1_234_567_890_000u64), 6),
            "1,234,567.89"
        );
        assert_eq!(fmt_units(U256::from(5), 18), "0");
        assert_eq!(
            fmt_signed(I256::try_from(-2_500_000i64).unwrap(), 6),
            "-2.5"
        );
    }
}
