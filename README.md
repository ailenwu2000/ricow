# ricow

A fully local, multi-platform quantitative strategy engine (backtest / Dry Run / live trading). Pure local CLI — no cloud, no telemetry, no auto-update.

> Formerly named **locus**; renamed to **ricow** in 2026-09. The Git history was not carried over (this repository starts fresh at `v0.1.0`).

## Quick start

### 1. Build

```bash
cargo build --release        # artifact: target/release/ricow
```

Data directory (`$RICOW_ROOT`) resolution order: env var `RICOW_ROOT` → current directory (if it contains `ricow.db` / `strategies/`) → platform default data directory.

### 2. Configuration: exactly one file

All config and credentials live in `$RICOW_ROOT/ricow.toml` (mode `0600`, already in `.gitignore`). **The tool never writes your keys for you**: if the file is missing, the first command that needs it generates a commented template and tells you the path — you fill in the values with your editor (a key sits in the same `[ai]` section as its provider, so you can't mix them up).

```toml
[ai]                     # optional: only needed for the built-in AI assistant
provider = "deepseek"    # built-in preset (deepseek/moonshot/zhipu/qwen/openrouter/openai/ollama), or any custom name (then base_url is required)
model = "deepseek-chat"  # for a self-hosted/proxy endpoint, add: base_url = "..."
api_key=***              # key of that provider (same section as provider)

[exchange]               # optional: only needed for demo / live order placement
demo_key=***  demo_secret=***            # Binance demo (testnet) credentials
binance_key=***  binance_secret=***      # Binance mainnet credentials (real money)
```

**How to get a demo key**: Binance's simulated-trading (demo) platform <https://demo.binance.com/> has its own account system (separate from mainnet). Create an API key there — enable **trading only**, **never** withdrawals. Note that demo keys are **not** interchangeable with the legacy spot testnet `testnet.binance.vision`. Mainnet keys are created the same way at <https://www.binance.com> (also recommended: disable withdrawals and restrict by IP).

### 3. Four run modes

| Mode | Command | Capital | Prerequisites |
|---|---|---|---|
| Backtest | `ricow backtest --strategy shannon_grid --pair ETHUSDT --days 30` | none | zero config |
| Dry Run (local virtual matching) | `ricow start <name>` | none | zero config |
| demo (Binance testnet, **real orders**) | `ricow start <name> --demo` | simulated | `[exchange].demo_*` |
| Live (mainnet, **real money**) | strategy TOML declares `live_enabled = true` + `ricow start <name> --live --accept-risk` | real | `[exchange].binance_*` + first-use risk acknowledgement + Dry Run duration gate |

demo and live both hit the real exchange API — they differ only in domain (`demo-api`/`demo-fapi` vs mainnet) and credentials, and the two credential sets **never fall back to each other**. The two live-only gates (risk acknowledgement / minimum Dry Run duration) do not apply to demo, because they protect real money.

Inspect: `ricow list` / `ricow info <name>`; stop: `ricow stop <name> [--close-all]` (residual open orders / positions are reported from the exchange's actual state).

### 4. Built-in AI assistant (optional)

`ricow ai "which strategies do I have deployed?"` — natural-language status queries, backtests, and authoritative doc lookups (all **read-only**). Anything that writes to disk, starts live trading, or closes a position is **not in the tool surface** — only you can confirm it by typing the command yourself. `--plain` disables streaming output.

### 5. Use your own AI agent (optional)

If you already use Claude Code / Codex / Cursor, you don't need the built-in assistant:

```
ricow agent-kit --install        # generate the manual in the current directory (a directory can be given too)
ricow agent-kit                  # just print the manual to the terminal
```

It writes 4 files: `AGENTS.md` (operating manual) / `SKILL.md` (Agent Skills format) / `CLAUDE.md` (one-line import) / `lua-api.md` (strategy API reference). The manual shares its source with the built-in assistant's system prompt and states that write actions (deploy / go live / close positions / change params) **must be executed by you in a terminal**. Existing files with different content are **refused**, writing nothing, so your own `AGENTS.md` is never clobbered.

### 6. Network and data sources

ricow **always needs to reach Binance** (public market data + signed endpoints). From mainland China use a proxy/VPN — the CLI uses `reqwest` and honours the standard proxy env vars:

```
export HTTPS_PROXY=http://127.0.0.1:7890   # your proxy (or HTTP_PROXY / ALL_PROXY)
```

- On `network error`, first check the proxy: `curl https://api.binance.com/api/v3/time` should return JSON.
- `RICOW_BN_BASE_URL` / `RICOW_FAPI_BASE_URL` replace the whole REST domain (spot / futures): **public data and signed orders both move with it**; Binance's public-data-only domain `https://data-api.binance.vision` has **no trading endpoints** — fine for pure backtests, but orders will fail there.
- demo (`--demo`) needs no domain config: the CLI uses `demo-api.binance.com` / `demo-fapi.binance.com`.

### 7. Security notes

- Credentials are stored in **plaintext** in `ricow.toml` (0600, local-only, never committed): encryption only moves the problem ("where do you keep the decryption key?" — same file = security theatre, machine fingerprint = breaks on hardware change), and comparable tools (freqtrade / jesse / Claude Code) also store plaintext.
- Never share your keys with anyone (including AI assistants) and never commit them.
- Go live small, and only after Dry Run and demo.
- Write confirmations (deploy `确认部署 <name>` / live `确认实盘 <name>`) **can only be typed in an interactive terminal**: phrases fed through pipes, scripts, or AI-agent tool calls are always rejected.

## Repository layout

- `specs/` — the single documentation tree: constitution / product / architecture / lua-api / roadmap / research / changes (SDD artefacts)
- `crates/` — 5-crate workspace: core / binance / strategy / engine / cli
- `strategies/builtin/` — built-in references: `shannon_grid.lua` (strategy template) + `executors/{dca,twap,vwap,pullback,ladder}` (execution-pattern examples, not strategies); the `exec` components are built into the engine (Rust) and called from Lua via `exec.*`
- `examples/` — user strategy template and examples (`strategy_template.toml` / `ema_cross.lua`)
- `crates/ricow/src/supervisor/` — strategy process manager (resident daemon + local control channel + instance ledger; see [specs/architecture.md §三](specs/architecture.md))

## Platform support

| Platform | Status |
|---|---|
| Linux x86_64 | Verified on a real machine (build, backtests, demo/testnet flows) |
| macOS / Windows | Cross-platform code paths exist (`cfg(unix)` / `cfg(windows)`) and are covered by CI build + unit tests; live/Dry Run flows have **not** been exercised on real hardware yet |

## Disclaimer

This software is for learning and research only and is not investment advice. Cryptocurrency trading carries extreme risk. You are solely responsible for any use of it with real funds.

---

[中文文档](README_zh.md) · [项目宪法 / constitution](specs/constitution.md) · [贡献指南 / Contributing](CONTRIBUTING.md)
