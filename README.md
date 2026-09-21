# ricow

<img src="website/logo.svg" alt="ricow" width="96">

**English** | [中文文档](README_zh.md)

A fully local, multi-platform quantitative strategy engine (backtest / Dry Run / live trading). Pure local CLI — no cloud, no telemetry, no auto-update.

**From idea, to rule, to live.**

> Formerly named **locus**; renamed to **ricow** in 2026-09. The Git history was not carried over (this repository starts fresh at `v0.7.0`).

## Install

Prebuilt self-contained archives are published on the [releases page](https://github.com/ailenwu2000/ricow/releases) — **no Rust toolchain needed**. Four installers are generated from the same configuration: shell, PowerShell, Homebrew and msi.

**Linux / macOS — shell installer**

```bash
curl -LsSf https://github.com/ailenwu2000/ricow/releases/latest/download/ricow-installer.sh | sh
```

**Windows — PowerShell installer**

```powershell
irm https://github.com/ailenwu2000/ricow/releases/latest/download/ricow-installer.ps1 | iex
```

**Windows — msi**

Download `ricow-x86_64-pc-windows-msvc.msi` from the release page and double-click it. It installs `ricow.exe` under `%ProgramFiles%\ricow\bin` and appends that directory to `PATH` — open a **new** terminal so the change is picked up. The msi carries the binary only; the double-click entry below comes with the `.zip`.

**macOS — Homebrew**

```bash
curl -LO https://github.com/ailenwu2000/ricow/releases/latest/download/ricow.rb
brew install ./ricow.rb
```

There is **no tap** yet, so `brew install ricow` does not work: the formula itself is published as a release artifact and installs the prebuilt binary (nothing is compiled).

**Any platform — manual archive**

Download the archive matching your platform, extract it, and run `./ricow` (Windows: `ricow.exe`). Every archive also carries a launcher for the built-in assistant: `启动-ricow-AI助手.cmd` on Windows (double-click it; it switches the console to UTF-8, which is what keeps the Chinese output readable), `启动-ricow-AI助手.sh` on Linux/macOS (`./启动-ricow-AI助手.sh`), and `启动-ricow-AI助手.command` on macOS, which is the same thing in a double-clickable wrapper. The launchers go through ricow's bare entry point, so the first run asks you *in the conversation* for the provider and the API key (typed silently, never echoed) and writes them into `ricow.toml` — no environment variable to set beforehand. For the web version, double-click `启动-ricow-Web.cmd` / `启动-ricow-Web.command` (`./启动-ricow-Web.sh` on Linux/macOS) instead: it starts the local web page and opens your browser — see §5.

**Updates.** Nothing we ship contains an auto-updater: the app never rewrites itself from the network, so upgrading is your explicit action — download the newer archive/msi or re-run the installer. `winget` and `scoop` packages are **not** provided; use the PowerShell installer, the msi, or the `.zip`.

## Quick start

### 1. Build from source (optional — skip this if you installed a release)

```bash
cargo build --release        # artifact: target/release/ricow
```

Data directory (`$RICOW_ROOT`) resolution order: env var `RICOW_ROOT` → current directory (if it contains `ricow.db` / `strategies/`) → platform default data directory.

From a checkout you start the assistant exactly the way the archives do: double-click `packaging\启动-ricow-AI助手.cmd` on Windows, or run `packaging/启动-ricow-AI助手.sh` on Linux/macOS. It looks for a binary next to itself first, then in `target/` (whichever of `release`/`debug` was built last), then on `PATH`; when it falls back to `target/` it also `cd`s to the repository root, so the assistant uses this checkout's `strategies/` instead of a second, empty data directory. `cargo ai` is a repository-local alias for `cargo run -p ricow -- ai` (`.cargo/config.toml`) — `ai` is a ricow subcommand, not a cargo one, so that spelling only exists inside this checkout. The web version works the same way: double-click `packaging\启动-ricow-Web.cmd` / run `packaging/启动-ricow-Web.sh`, or simply `cargo run -p ricow -- web`.

### 2. Configuration: exactly one file

All config and credentials live in `$RICOW_ROOT/ricow.toml` (already in `.gitignore`; on Unix it is written `0600` — Windows has no POSIX mode, so there the write narrows the file to the current user via ACL). If the file is missing, the first command that needs it generates a commented template and tells you the path. Fill it in with your editor — or from inside `ricow` (see §4) use `/keys` (masked input, never echoed) and `/market` to change the trading-pair view; those two edit the file **in place, keeping your comments**, and only touch a 9-key whitelist. A key sits in the same section as its provider, so you can't mix them up.

```toml
[ai]                     # optional: only needed for the built-in AI assistant
provider = "deepseek"    # built-in preset (deepseek/moonshot/zhipu/qwen/openrouter/openai/ollama), or any custom name (then base_url is required)
model = "deepseek-flash" # for a self-hosted/proxy endpoint, add: base_url = "..."
api_key=***              # key of that provider (same section as provider)

[exchange]               # optional: only needed for demo / live order placement
demo_key=***                  # Binance demo (testnet) credentials
demo_secret=***
binance_key=***               # Binance mainnet credentials (real money)
binance_secret=***
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

`ricow ai "which strategies do I have deployed?"` — natural-language status queries, backtests, and authoritative doc lookups (all **read-only**). Running bare `ricow` opens the same assistant as an interactive session (`chat`); on a fresh install it walks you through first-run setup instead (Chinese or English). Anything that writes to disk or changes a running state — deploy, replace, edit params, delete, start/stop dry run, demo & live — is **not in the tool surface**: the model can only register a pending action, and the host runs it only after **you** confirm. In the chat session a single word in the session language is enough (`确认` / `confirm`); terminal commands keep the exact verbatim phrase (e.g. `确认实盘 <name>`). Slash commands: `/help` `/history` `/lang` `/keys` `/market` `/exit`. `--plain` disables streaming output.

### 5. Web UI (optional)

`ricow web` starts a built-in web page on your machine's `127.0.0.1` (random port), prints the URL with a one-time token, and tries to open your browser; from then on every conversation and action happens in the page. **The server is the very same session engine as the CLI**, just with a second front end: conversation history on the left (create / switch / delete; reopening an old session restores the last 20 turns as context), the transcript above and the input box below on the right; keywords / warnings / errors are coloured by severity, and quant terms open an explanation when clicked; the UI toggles between Chinese and English (sharing `ricow.toml`'s `[ui].lang` with the CLI). The token is random per launch and **never written to disk or logs**, and the page plus every API endpoint sit behind it (missing or wrong token always gets a `401` with no session content in the body) — in other words it is **local-only** and never exposed to your LAN or the internet.

The archives ship double-click launchers too: `启动-ricow-Web.cmd` on Windows, `启动-ricow-Web.command` on macOS (or `./启动-ricow-Web.sh`), `./启动-ricow-Web.sh` on Linux — same lookup logic as the assistant launchers, with the start line swapped to `ricow web`.

### 6. Use your own AI agent (optional)

If you already use Claude Code / Codex / Cursor, you don't need the built-in assistant:

```
ricow agent-kit --install        # generate the manual in the current directory (a directory can be given too)
ricow agent-kit                  # just print the manual to the terminal
```

It writes 4 files: `AGENTS.md` (operating manual) / `SKILL.md` (Agent Skills format) / `CLAUDE.md` (one-line import) / `lua-api.md` (strategy API reference). The manual shares its source with the built-in assistant's system prompt and states that write actions (deploy / go live / close positions / change params) **must be executed by you in a terminal**. Existing files with different content are **refused**, writing nothing, so your own `AGENTS.md` is never clobbered.

### 7. Network and data sources

ricow **always needs to reach Binance** (public market data + signed endpoints). From mainland China use a proxy/VPN — the CLI uses `reqwest` and honours the standard proxy env vars:

```
export HTTPS_PROXY=http://127.0.0.1:7890   # your proxy (or HTTP_PROXY / ALL_PROXY)
```

- On `network error`, first check the proxy: `curl https://api.binance.com/api/v3/time` should return JSON.
- `RICOW_BN_BASE_URL` / `RICOW_FAPI_BASE_URL` replace the whole REST domain (spot / futures): **public data and signed orders both move with it**; Binance's public-data-only domain `https://data-api.binance.vision` has **no trading endpoints** — fine for pure backtests, but orders will fail there.
- demo (`--demo`) needs no domain config: the CLI uses `demo-api.binance.com` / `demo-fapi.binance.com`.
- **Built-in data sources** (`ricow data pull --source <name>`): `binance_spot` / `binance_futures` (K-line ranges with pagination) and `nasdaq` / `yahoo` (daily US stocks, no API key). Everything lands in one local table (`data_klines`), keyed by `(source, symbol, interval, open_time)`.
- **Backtests read the local database only** (reproducibility): pull first (`ricow data pull --source binance_spot --symbol ETHUSDT --interval 1h --days 150`), then `ricow backtest …`. If data is missing, the error prints the exact `data pull` command to run — no hidden network fetch mid-backtest.
- **Your strategy may fetch its own data**: Lua strategies call `http:get(url)` (GET only, 10s wall-clock timeout, 5MB response cap). Any URL/domain is allowed — you own the risk; built-in strategies never use it.

### 8. Security notes

- Credentials are stored in **plaintext** in `ricow.toml` (local-only, never committed; `0600` on Unix, current-user-only ACL on Windows): encryption only moves the problem ("where do you keep the decryption key?" — same file = security theatre, machine fingerprint = breaks on hardware change).
- Never share your keys with anyone (including AI assistants) and never commit them.
- Go live small, and only after Dry Run and demo.
- Write confirmations are **typed by you, verbatim, in an interactive terminal** — deploy `确认部署 <name>` / start demo `确认启动测试网 <name>` / first-use risk acknowledgement `确认风险` / go live `确认实盘 <name>` / stop demo `确认停止测试网 <name>` / stop live `确认停止实盘 <name>` / close positions and stop `确认平仓停止 <name>`. Phrases fed through pipes, scripts, or AI-agent tool calls are always rejected, and a bare `y`/`yes`/Enter never counts. Going live still re-runs the host-side gates (risk acknowledgement → Dry Run duration → clock pre-check) unchanged.

## FAQ

**Chinese output is mojibake / boxes in the Windows console.** The console code page is not UTF-8. Either run `chcp 65001` in that terminal first, or start through `启动-ricow-AI助手.cmd`, which does it for you (that is the whole reason the file exists). To fix it once for all consoles, turn on Windows 11's *Settings → Time & language → Language & region → Administrative language settings → Beta: Use Unicode UTF-8 for worldwide language support*.

**Windows: "Windows protected your PC" / SmartScreen blocked it.** We do not ship a code-signing certificate (a deliberate decision, explained below). Click *More info → Run anyway*, or unblock the extracted files once and stop the prompts:

```powershell
powershell -Command "Get-ChildItem -Recurse .\ | Unblock-File"
```

**macOS: "cannot be opened because the developer cannot be verified" (or "is damaged").** That is Gatekeeper, not a broken download: the binaries are neither signed nor notarized. Drop the quarantine flag:

```bash
xattr -d com.apple.quarantine ./ricow
```

or use *System Settings → Privacy & Security → Open Anyway*. **Why we don't sign:** an Apple Developer certificate is a paid subscription and a Windows certificate requires an organization, while the whole point of this project is "build it locally and trust the source you read" — `cargo build --release` gives you a binary nothing has to vouch for.

**Linux/macOS: the launcher reports `Permission denied`.** Running or double-clicking a launcher needs the executable bit; archives normally keep it, and if it was lost, restore it once with `chmod +x ricow 启动-ricow-AI助手.sh 启动-ricow-AI助手.command` (the script prints that same hint when it finds `./ricow` present but not executable).

**How do I switch AI provider, or use a local model?** Edit `[ai]` in `ricow.toml`: `provider` (presets: `deepseek` (default) / `moonshot` / `zhipu` / `qwen` / `openrouter` / `openai` / `ollama`), `model`, and for anything not on that list also `base_url`. The provider's `api_key` lives in the same section. From inside the assistant, `/keys` writes provider + key + model back into the file while keeping your comments.

**Can I run it offline?** The AI part can be: `provider = "ollama"` with `base_url = "http://127.0.0.1:11434/v1"` and a model you already pulled (`ollama list`) needs no internet at all. **Backtests can too**: pull the data first (`ricow data pull`), then backtests read only the local database — that is what makes them reproducible. demo/live place real orders, so Binance must be reachable (from mainland China, set `HTTPS_PROXY`). Only the AI endpoint is optional.

**Where do my strategies, database and config live?** In `$RICOW_ROOT` — resolution order: the `RICOW_ROOT` env var → the current directory if it already contains `ricow.db`/`strategies/` → the platform default data directory. `ricow.toml` (`0600` on Unix / current-user ACL on Windows) and `ricow.db` both live there.

**How big is the download, and how long does a build take?** Measured on the maintainer's machine, 2026-09-17, reported as measured rather than predicted: the `dist`-profile `ricow.exe` is 21,042,176 bytes (20.1 MiB) and the Windows `.zip` of `ricow.exe` + `README.md` + `LICENSE` is 8,614,204 bytes (≈8.2 MiB); a from-scratch build of that profile (cold cache) took 284.5 s (4m44s). Plain `cargo build --release` produces a 19,930,624-byte (≈19.0 MiB) binary. One machine, one measurement — treat these as an order of magnitude.

## Repository layout

- `specs/` — the single documentation tree: constitution / product / architecture / lua-api / roadmap / research / changes (SDD artefacts)
- `crates/` — 5-crate workspace: core / binance / strategy / engine / cli
- `strategies/builtin/` — built-in references, compiled into the binary: `shannon_grid.lua` (**the only strategy template** — read it as your starting point) + `executors/{dca,twap,vwap,pullback,ladder}` (execution-pattern examples, not strategies); the `exec` components live in the engine (Rust) and are called from Lua via `exec.*`
- Writing your own strategy: `ricow create --name <name> --pair <pair> --script <file.lua>` → `ricow approve` → `ricow deploy <preview_id> --token <token>` (compile gate + real-K-line sandbox backtest; nothing is written to disk until you confirm in an interactive terminal) — see [specs/lua-api.md](specs/lua-api.md) §九
- `website/` — the landing page served at <https://ricow.xyz> (plain static HTML/CSS, deployed to GitHub Pages by `.github/workflows/pages.yml`)
- `crates/ricow/src/supervisor/` — strategy process manager (resident daemon + local control channel + instance ledger; see [specs/architecture.md §三](specs/architecture.md))

## Platform support

| Platform | Status |
|---|---|
| Linux x86_64 | Verified on a real machine (build, backtests, demo/testnet flows) |
| Linux arm64 | Built and published by CI; not exercised on real hardware |
| Windows x86_64 | Builds and the full unit-test suite pass in CI (`test (windows-latest)`, 2026-09-15); live/Dry Run flows have **not** been exercised on real hardware yet |
| macOS | Not verified — no hardware available and no CI coverage yet; the code takes the `cfg(unix)` path shared with Linux |

The four installers are generated from a single configuration, but only the shell installer has been exercised on a real machine (Linux); the PowerShell installer, the msi and the Homebrew formula have **not** been installed on real Windows/macOS hardware yet. What *is* verified locally: `dist plan` lists shell + PowerShell + Homebrew + msi, every archive carries `LICENSE`/`README.md`/`启动-ricow-AI助手.cmd`/`启动-ricow-AI助手.sh`/`启动-ricow-AI助手.command`, and no artifact contains an updater.

## Disclaimer

This software is for learning and research only and is not investment advice. Cryptocurrency trading carries extreme risk. You are solely responsible for any use of it with real funds.

---

[中文文档](README_zh.md) · [项目宪法 / constitution](specs/constitution.md) · [贡献指南 / Contributing](CONTRIBUTING.md)

### Restricted network

取数走标准环境变量代理(平台不提供代理配置项):

```bash
HTTPS_PROXY=http://127.0.0.1:1080 ricow data pull --source yahoo --symbol QQQ --interval 1d --days 3650
```

Backtests read the local store only and need no network. **Fetch failures fail loudly** (no silent fallback to stale local data); behind a restricted network, pass a proxy env var to the fetch command as shown above.
