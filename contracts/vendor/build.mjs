// Rebuilds contracts/vendor/artifacts from the pinned v2-core and v3-core
// submodules using the compiler settings Uniswap shipped with.
//
// The old compilers (0.5.16, 0.7.6) have no native Apple Silicon builds, so
// this uses solc-js, which is the same compiler compiled to wasm. The build
// fails unless the pair and pool init code hashes equal the mainnet ones,
// which proves the bytecode is byte-for-byte what Uniswap deployed.
//
//   cd contracts/vendor && npm ci && npm run build

import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const contractsDir = path.resolve(here, '..');
const outDir = path.join(here, 'artifacts');

// node:crypto has sha3-256 but not Ethereum's pre-standard keccak256.
const { keccak256 } = require('js-sha3');

const hashCode = (bytecode) => '0x' + keccak256(Buffer.from(bytecode, 'hex'));

const V2_PAIR_INIT_CODE_HASH = '0x96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f';
const V3_POOL_INIT_CODE_HASH = '0xe34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54';

function collect(dir, base, filter = () => true) {
  const out = {};
  const walk = (d) => {
    for (const f of fs.readdirSync(d).sort()) {
      const p = path.join(d, f);
      if (fs.statSync(p).isDirectory()) walk(p);
      else if (p.endsWith('.sol') && filter(p)) {
        out[path.relative(base, p).split(path.sep).join('/')] = { content: fs.readFileSync(p, 'utf8') };
      }
    }
  };
  walk(dir);
  return out;
}

function compile(solc, sources, settings, findImports) {
  const input = {
    language: 'Solidity',
    sources,
    settings: { ...settings, outputSelection: { '*': { '*': ['evm.bytecode.object'] } } },
  };
  const out = JSON.parse(solc.compile(JSON.stringify(input), findImports ? { import: findImports } : undefined));
  const errors = (out.errors || []).filter((e) => e.severity === 'error');
  if (errors.length) {
    for (const e of errors) console.error(e.formattedMessage);
    process.exit(1);
  }
  return out.contracts;
}

function write(name, meta, bytecode) {
  const artifact = { contractName: name, ...meta, bytecode: '0x' + bytecode };
  fs.writeFileSync(path.join(outDir, `${name}.json`), JSON.stringify(artifact, null, 2) + '\n');
  console.log(`${name}: ${bytecode.length / 2} bytes, keccak ${hashCode(bytecode)}`);
}

function expectHash(name, bytecode, expected) {
  const got = hashCode(bytecode);
  if (got !== expected) {
    console.error(`${name} init code hash ${got} != mainnet ${expected}`);
    process.exit(1);
  }
}

fs.mkdirSync(outDir, { recursive: true });

// Uniswap V2 core: solc 0.5.16, istanbul, 999999 runs (v2-core/.waffle.json).
// Source keys must be "contracts/..." because the metadata hash covers them.
{
  const solc = require('solc-0.5.16');
  const root = path.join(contractsDir, 'lib', 'v2-core');
  const sources = collect(path.join(root, 'contracts'), root, (p) => !p.includes(`${path.sep}test${path.sep}`));
  const settings = { evmVersion: 'istanbul', optimizer: { enabled: true, runs: 999999 } };
  const c = compile(solc, sources, settings);
  const meta = { source: 'lib/v2-core', compiler: solc.version(), settings };
  const pair = c['contracts/UniswapV2Pair.sol'].UniswapV2Pair.evm.bytecode.object;
  expectHash('UniswapV2Pair', pair, V2_PAIR_INIT_CODE_HASH);
  write('UniswapV2Pair', meta, pair);
  write('UniswapV2Factory', meta, c['contracts/UniswapV2Factory.sol'].UniswapV2Factory.evm.bytecode.object);
}

// Uniswap V3 core: solc 0.7.6, 800 runs, no metadata hash (v3-core/hardhat.config.ts).
const v3Settings = { optimizer: { enabled: true, runs: 800 }, metadata: { bytecodeHash: 'none' } };
{
  const solc = require('solc');
  const root = path.join(contractsDir, 'lib', 'v3-core');
  const sources = collect(path.join(root, 'contracts'), root, (p) => !p.includes(`${path.sep}test${path.sep}`));
  const c = compile(solc, sources, v3Settings);
  const meta = { source: 'lib/v3-core', compiler: solc.version(), settings: v3Settings };
  const pool = c['contracts/UniswapV3Pool.sol'].UniswapV3Pool.evm.bytecode.object;
  expectHash('UniswapV3Pool', pool, V3_POOL_INIT_CODE_HASH);
  write('UniswapV3Pool', meta, pool);
  write('UniswapV3Factory', meta, c['contracts/UniswapV3Factory.sol'].UniswapV3Factory.evm.bytecode.object);
}

// Math harness over the same v3-core libraries and settings.
{
  const solc = require('solc');
  const key = 'vendor/V3MathHarness.sol';
  const sources = { [key]: { content: fs.readFileSync(path.join(contractsDir, key), 'utf8') } };
  const findImports = (p) => {
    const file = path.join(contractsDir, p);
    return fs.existsSync(file) ? { contents: fs.readFileSync(file, 'utf8') } : { error: `not found: ${p}` };
  };
  const c = compile(solc, sources, v3Settings, findImports);
  const meta = { source: 'vendor/V3MathHarness.sol', compiler: solc.version(), settings: v3Settings };
  write('V3MathHarness', meta, c[key].V3MathHarness.evm.bytecode.object);
}
