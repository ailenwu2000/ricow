# P4 dogfood 执行清单 (真实小资金实盘闭环)

**版本**: 2026-09-13 初稿 · **2026-10-18 全面校正**(对照最新代码逐条修订, 见文末「八之订正」) | **适用**: ricow 011(Dry Run/实盘运行器)+ 002(建策略入口与 Dry Run 时长门禁)+ 003(出站通知)+ 013/014(合约记账与资金费/强平可见)+ **057(单笔名义护栏 / kill switch)**
**门槛来源**: `specs/roadmap.md` P4 =「dogfood 实盘 → 公开发布」, 发布判据 = **"可用且不会让用户莫名亏损"**。
**状态**: ⚠️ **本文档尚未真跑过**(阶段 0~3 全是待执行)。文中所有"实测"字样来自 2026-09 那轮预演, 当时 `[risk]` 护栏还存在 —— **那段护栏已于 2026-09-16 随 019-R5 整体删除**, 别再照抄。

---

## 零、纪律边界(先读, 别跳)

1. **仓库的测试纪律不变**: 宪法/`product.md` §十一 规定交易流程测试必须走币安 demo 测试网, **禁主网下单测试**。
   本清单是**产品级 dogfood(由用户本人执行的真实小资金运行)**, 不是 repo 测试 —— 因此它**不进 `cargo test`**,
   也不得因此往仓库里加"主网集成测试"。
2. **部署/系统级操作与资金操作由你本人执行**; 我(agent)只给步骤、命令与判据, 不代跑、不代持凭据。
3. **只用亏得起的钱**, 并把策略限制在**交易所单独子账号**里(product.md §十二 建议): 网格会吃掉账号内大部分现金,
   子账号隔离风险且独立核算。
4. **平台不做投资判断**: 护栏只有两条**固定工程护栏**(频率 100 单/秒 + 单笔名义 ≤ 报价资产权益×10),
   **没有**最大持仓 / 单日亏损 / 回撤熔断 —— 后三者属**投资判断**, 必须由策略用 `ctx:net_pnl()` / `ctx:equity()` 自管。
   所以「阶段 0/1/2 能验什么、验不了什么」要看清 §七之二。

---

## 一、一次性准备

| # | 事项 | 命令/做法 | 判据 |
|:--|:--|:--|:--|
| 1 | 交易所子账号 | 交易所后台建子账号, 转入**小资金** | ~~建议 200~500 USDT~~ → **已被实测证伪, 别照做**(见下方订正框)。下面示例仍按 300 写, 但那是 **`paired_grid`** 的口径, 不是 `shannon_grid` |
| 2 | API key 权限 | **只开"现货交易"**(合约另开"合约交易"); **关闭提现**; 建议绑 IP | 权限页截图留档 |
| 3 | 凭据入库(**只有阶段 3 实盘需要**) | 见下方「凭据怎么填」一节 —— **没有 `ricow keyring` 命令**(它与 OS Keyring、`credentials.toml` 已于 2026-09-14 一并移除) | `ricow.toml` 的 `[exchange]` 段已填, 或 env 已设。⚠️ **主网 Dry Run 与回测都不需要凭据**(只读公共行情); demo 真实调用需要 demo 专属凭据(见 `specs/testnet.md`); 实盘才需要主网凭据 |
| 4 | 时钟预检 | 见下方命令 | 本机超前 ≤1000ms、滞后 ≤4000ms(超了会被硬拒) |
| 5 | 出站通知(可选, 强烈建议) | 策略 TOML 里 `params.notify_webhook = "https://api.telegram.org/bot<TOKEN>/sendMessage"` + `notify_chat_id` | 实盘时熔断/成交/接近强平/停机残留会推到手机 |
| 6 | 数据目录 | `export RICOW_ROOT=<独立目录>`(避免与 demo 探针数据混在一起) | 该目录下会生成 `ricow.db` / `strategies/` / `logs/` |

> #### ⚠️ 订正 (2026-10-18)：本手册原建议的「200~500 USDT」**已被实测证伪**
>
> 同一内置策略 `shannon_grid`，两档本金真跑结果（demo 测试网阶段 0，干净 root `tmp/p4/`）：
>
> | 本金 | 结果 |
> |:--|:--|
> | `--cash 100000`（默认） | **21 笔成交**，净 +6.07，手续费 54.23 |
> | `--cash 300`（= 本手册原建议的小资金口径） | **只成交 1 笔**（建仓那一笔），之后一单挂不出，胜率 0% |
>
> 根因：建仓吃掉一半资金后只剩 149.85 USDT，**达不到下一档的最小名义**（现货 `minNotional` 5 USDT 与网格单笔额共同约束）。所以本金不足时策略是「**跑不起来**」，不是「收益差一点」—— 这与直觉相反，也是照做会直接踩空的地方。
>
> **怎么办**：
> ① **小资金请用 `paired_grid` 而非 `shannon_grid`** —— 前者按固定间距挂单、单笔金额小，小资金下能正常铺开（本手册下面示例按 300 设计的就是它的口径）；
> ② 或把 `shannon_grid` 的建仓比例与单笔金额**压到本金的 1%~2%** 再跑，并先用回测确认成交笔数不是个位数；
> ③ 无论哪种，**第一件事是跑一遍回测看「成交笔数」** —— 笔数是 1~2 就说明本金不够，再往下调参都是白调。
>
> 该结论已写进 AI 的提示词（`specs/changes/058-...`），AI 在帮你配策略时会按资金量主动提醒这件事。

### 凭据怎么填(2026-10-18 校正)

**当前唯一入口 = `$RICOW_ROOT/ricow.toml` 的 `[exchange]` 段**, 或 env 变量。键名:

| 用途 | `ricow.toml` 键名 | 环境变量 |
|:--|:--|:--|
| 主网实盘 | `binance_key` / `binance_secret` | `RICOW_BN_API_KEY` / `RICOW_BN_SECRET_KEY` |
| demo 测试网 | `demo_key` / `demo_secret` | `RICOW_DEMO_KEY` / `RICOW_DEMO_SECRET` |

**优先级**: 对应 env 成对出现时**优先于** `ricow.toml`(demo 侧如此; 主网侧 env 亦可覆盖)。
未填时 `ricow start --live` 会**拒绝并打印该填哪个文件哪个键**, 不是静默降级。

> ⚠️ **`ricow keyring` 与 `credentials.toml` 已彻底移除**(2026-09-14), `ricow setup` 也没有。
> 交互式录入走裸入口向导(`ricow` 不带子命令 → 供应商/密钥 → 外科式写回 `ricow.toml`),
> 或对话内 `/keys demo` / `/keys live`(静默录入, 只回显尾 4 位); Web UI 走「密钥」视图。
> **不要把密钥写进仓库里的 `ricow.toml`** —— 真跑密钥走 env 变量或仓库外文件。

### ⚠️ 关于 `[risk]` 段(2026-10-16 起全部失效)

本文档旧版本教你在策略 TOML 里写 `[risk]` 段配 `max_position_notional` / `max_daily_loss_usd` /
`min_order_notional` / `max_slippage_bps`。**这套已经不存在了**:

- 019-R5(2026-09-16)**删除了** `RiskEngine` 及其四条静态限额(最大持仓 / 单日亏损 / 最小订单 / 最大滑点);
- 老策略 TOML 里若仍留着 `[risk]` 段, serde **静默忽略未知段** —— **装载不报错、限额完全不生效**;
- 这正是「以为设了护栏其实没有」的最隐蔽失效方式。

**替代方案**: 仓促替代物不存在 —— 仓位/亏损政策应写在**策略脚本里**(用 `ctx:net_pnl()` / `ctx:equity()`
自管回撤/止损, 内置 `shannon_grid` 的 `dd_stop_pct` 是参考写法, 默认关闭)。平台侧只保留两条
**固定工程护栏**(频率 + 单笔名义), 无配置面。

时钟检查(WSL 漂移是常见坑, 实测出现过 +1120ms 被拒):

```bash
python3 -c "import time,json,urllib.request as u;print(int(time.time()*1000)-json.load(u.urlopen('https://api.binance.com/api/v3/time'))['serverTime'],'ms')"
# 绝对值持续 >500ms 就先修: Windows 侧 `wsl --shutdown` 重开, 或 sudo hwclock -s
```

---

## 二、阶段 0 — 回测基线(交易所无关, 先跑, 留档对照)

⚠️ **`start_price` 是必填参数**(`shannon_grid` 清单里 `required = true` 且**无默认值**)——
不显式给值会 `[FATAL] 缺少必填参数 start_price` 直接停机、**0 成交**, 别误读成"策略不work"。

```bash
# 取当前价当 start_price(手册里用一次实际价格, 你执行时换成当天的)
PX=$(curl -s https://api.binance.com/api/v3/ticker/price?symbol=ETHUSDT | "C:/Users/sunhuazhu/.workbuddy/binaries/python/versions/3.13.12/python.exe" -c "import sys,json;print(json.load(sys.stdin)['price'])")
ricow backtest --strategy shannon_grid --pair ETHUSDT --days 30 \
  --param start_price="$PX" --param invest_cash=300
```

**看什么**: `成交笔数` / `手续费占比`(现货网格实测约 **0.10%**/笔 —— 对应 taker 10bps)/ `最大回撤` / `拒单次数` / `净盈亏`。
**为什么**: 实盘跑起来后要拿真实成交的手续费与笔数**和这份基线对比** —— 差得多就是记账或撮合口径有问题(013/014/015 修的就是这类)。
> 注意: 回测窗口随当前时间滑动, **不同时刻跑出的数字不可直接比较**; 比较必须同一时刻连续跑两次或留档同一次的输出。
> `start_price` 按清单说明应设为**窗口起点价**, 上面用当前价是为了"立刻能跑起来"的第一轮基线;留档后建议改成起点价重跑一份正式基线。

---

## 三、阶段 1 — 建立待跑策略(两条等价路径)

**路径 A(手写, 推荐首跑用)**: 复制内置策略成自己的 id 再改 —— **别从零手写参数**, 内置清单里的
`type` 会让引擎自动注入编译期嵌入的脚本, 省掉 `script_path`。

```bash
# 在你的 RICOW_ROOT 下准备一份可改的副本
mkdir -p "$RICOW_ROOT/strategies/spot"
cp strategies/spot/shannon_grid.lua   "$RICOW_ROOT/strategies/spot/shannon-dogfood.lua"
cp strategies/spot/shannon_grid.toml "$RICOW_ROOT/strategies/spot/shannon-dogfood.toml"
```

> ⚠️ **2026-10-18 订正**: 本文旧版的模板用了 `order_size` / `target_ratio` / `rebalance_band`
> 三个键 —— **`shannon_grid` 清单里没有这三个参数**(真实键见下表), 照抄会得到"策略参数全是空转"的配置。
> 且 `initial_cash` 不是策略参数而是**引擎侧 Dry Run 参数**。
> 权威 schema = `strategies/spot/shannon_grid.toml` 的 `[[params]]`, 也可直接 `ricow run <name>` 后看界面渲染的表单。

> ✅ **Dry Run 虚拟本金可配**(`params.initial_cash`, 016 落地, 缺省 100000)——
> 把它设成**与实盘相同的资金口径**, 同一套配置就能同时预演 Dry Run 与实盘。
> (016 之前 Dry Run 本金硬编码 100k: 实测用小资金跑 Dry Run 会 `65 单全被拒`。)

清单文件 `strategies/spot/shannon-dogfood.toml`(**只改 `id` / `name`, 参数键一律照抄内置的**):

```toml
# 策略清单(manifest): 展示元数据 + 参数 schema。仅供 UI/CLI/AI, 引擎不读(架构铁律)。
id = "shannon-dogfood"
name = "香农网格(dogfood 实盘验证用)"
market = "spot"
summary = "复制自内置 shannon_grid, 仅改 id 与名称; 参数 schema 与内置一致"
description = "P4 dogfood 用的实盘验证副本。不要在这份清单里加内置没有的参数键 —— 引擎不读清单, 加了也不生效。"
suitable = "震荡市"
unsuitable = "单边急涨、极低波动"

# ↓ 以下 [[params]] 段整体照抄 strategies/spot/shannon_grid.toml, 不要改键名
```

**`shannon_grid` 的真实参数键(照抄这份, 别用旧文档里的键)**:

| 键 | 类型 | 必填 | 默认 | 说明 |
|:--|:--|:--:|:--|:--|
| `pair` | string | **是** | — | 交易对, 如 `ETHUSDT` |
| `start_price` | f64 | **是** | — | 价格低于它才激活。**无默认值, 不给会 FATAL 停机** |
| `invest_cash` | f64 | 否 | 0 | 投入资金(计价币), 激活时用其一半市价建仓。0 = 激活时 quote 余额全额 |
| `interval` | string | 否 | `1h` | 主时钟周期 |
| `atr_interval` | string | 否 | `1h` | ATR 用哪根 K 线 |
| `atr_period` | i64 | 否 | 14 | ATR 回看根数 |
| `atr_mult` | f64 | 否 | 1 | 网格间距 = `atr_mult × ATR` |
| `min_spacing_pct` | f64 | 否 | 0.004 | 最小间距(价格占比), 防 ATR 过窄被手续费磨损 |
| `min_notional` | f64 | 否 | 5 | 单笔最小名义金额 |
| `fee_side` | f64 | 否 | 0.001 | 单边费率(0.001 = 0.1%/边) |

> **`min_notional` 取代了旧的 `[risk].min_order_notional`** —— 它是策略自己的参数、**真生效**。
> 教训见 §七之四第1 条: 设得太大(比如 20 USDT 而单笔只有 ~19.6)会让策略每个 tick 都被拒一次。

**三种口径的资金来源(容易混, 单独说清)**:

| 模式 | 资金/权益从哪来 | 该按什么设 |
|:--|:--|:--|
| 回测 | CLI `--cash`(缺省 100000) | 按回测假设的本金 |
| **Dry Run** | `params.initial_cash`(缺省 100000; 016 可配) | 设成与实盘同一口径即可复用同一份配置 |
| **实盘 / demo** | **交易所账户权益**(与 `initial_cash` 无关) | 单笔名义护栏按**该账户实际权益 × 10** 自动算 —— 权益取不到即放行 |

> **平台侧护栏只有两条**(2026-10-18 后的真实状态):
> ① **下单频率 ≤ 100 单/秒**(滑动窗口, 计数对全部 pair 共享);
> ② **单笔名义 ≤ 报价资产权益 × 10**(`order_guard::NOTIONAL_LIMIT_MULTIPLE = 10`, 固定常量不进配置面,
>    **只在 Dry Run 与实盘生效**, 回测不装) —— 权益**只取报价资产余额不加持仓市值**。
> 两者拒单都返回 `Rejected` ack、计入"拒单次数", Dry Run/实盘还会落 `order_guard` 事件。
> **没有**最大持仓 / 单日亏损 / 滑点 / 回撤熔断。

**路径 B(AI/提交式)**: `ricow create --name shannon-dogfood --pair ETHUSDT --param order_size=0.01 --script <file.lua>`
→ `ricow approve <preview_id>` → `ricow deploy <preview_id> --token <t>`。部署物是 `strategies/<name>.toml` + `strategies/<market>/<name>.lua`(脚本落**市场子目录**, 实例 TOML 留根)。

---

## 四、阶段 2 — Dry Run(真实行情, 虚拟成交), **跑满 24 小时**

```bash
ricow run shannon-dogfood            # 前台; 停机: 输入 stop / Ctrl-C / 关管道
# 或后台: ricow daemon start 然后 ricow start shannon-dogfood
```

**为什么 24 小时**: 覆盖亚/欧/美三个交易时段的一个完整日周期 —— 网格策略的错误(报价、对齐、再平衡频率)在单一时段看不出来。
这是 002 加的**硬门禁**: 首次 Dry Run 启动会把起点写进策略 TOML(`dry_run_started_at = "…Z"`), 实盘启动时按
`params.min_dry_run_hours`(默认 24)核验, 不足直接拒。
> 若你确定要跳过(知情选择): 在 `[strategy.params]` 里设 `min_dry_run_hours = 0`。

**Dry Run 的两条重要语义**(实测确认):
- **每次启动都从零开始**: 虚拟本金 = `initial_cash`, 持仓为空 → 会重新建仓。**别把多次运行的成交混在一起看**。
- **`ricow fills` 跨运行累积**(按 `strategy_id` 存, 不区分运行批次)—— 对照时按时间戳切分你关心的那一轮。

**期间每天看一次**:
- `ricow info shannon-dogfood` —— 模式/成交统计/日志路径
- `ricow fills shannon-dogfood` —— 虚拟成交明细与手续费合计
- `ricow logs shannon-dogfood --lines 50` —— 有无 `WARN`/`ERROR`; 有无 `order_guard` 护栏拒单
- 判据: 有成交、无异常拒单、现金与持仓守恒(总价值≈初始 ± 行情波动, 不凭空增减)

---

## 五、阶段 3 — 实盘小跑(**你的决策点**)

**先做一次性风险确认**(018): 首次实盘、以及首次 `start --live`, 不带 `--accept-risk` 会被**拒绝**并打印风险披露要点 ——
读过之后在命令后加一次 `--accept-risk` 即可(`ricow run <name> --live --accept-risk`), 确认记入 `$RICOW_ROOT/risk_ack.json`, 之后不再要求。

**再开闸**: 把 TOML 的 `live_enabled = false` 改成 `true`(双条件门禁: 配置声明 **且** 命令行 `--live`, 缺一即按 Dry Run 跑并告诉你原因)。

```bash
ricow start shannon-dogfood --live      # 推荐: daemon 托管, 日志落 $RICOW_ROOT/logs/
# 或前台: ricow run shannon-dogfood --live
```

启动时会依次过: **Dry Run 时长门禁** → **时钟预检** → 凭据加载 → 下单参数对齐(按 step/tick/minNotional 自动对齐并打印)。

**首小时盯这几项**:
- 有没有 `order_guard` 拒单 / `Rejected`(预期: 0 —— 拒单要么是参数设得太紧, 要么是策略在乱下单)
- `ricow fills`: 真实成交的**手续费**是否与费率相符(现货 taker 10bps)
- 交易所网页端对照: 挂单/持仓/成交与 `ricow info` 是否一致
- WSL 漂移: 若出现 `-1021` 类签名错误, 先跑时钟检查再重启

**收工(每周/每轮结束时)**:
```bash
ricow stop shannon-dogfood --close-all   # 撤本实例挂单 + 市价平掉策略持仓
# 不带 --close-all = 只撤挂单, 持仓保留(想继续持有时用)
ricow info shannon-dogfood               # 复查: 残留挂单=0、持仓=0(若用了 --close-all)
```
> 停机清理只撤**本策略前缀**的挂单(`<策略名>-…`), 非本实例的单只上报不撤; 有残留会如实打印并推送到通知通道。

---

## 六、立即停手条件(任一命中就 `stop --close-all` 并回来复盘)

| 现象 | 可能原因 |
|:--|:--|
| 策略自管的止损/回撤阈值被触发(如 `shannon_grid` 的 `dd_stop_pct`, **默认关闭**) | 策略在持续亏损 —— 先看是行情还是 bug。**注意这不是平台护栏**: 平台已无两级亏损熔断(019-R5 删除), 该类阈值只写在策略里 |
| 拒单数持续增长 | `min_notional` 等策略参数设得太紧 / 策略在反复下无效单(如零动作单) |
| 账目对不上(交易所成交 ≠ `ricow fills`) | 成交回写或记账缺陷 —— 停止并留存日志 |
| 单笔名义超权益 × 10 被拒(`order_guard`) | 单笔金额数量级写错(把 1000 当 0.1、漏小数点)。**这正是该护栏的设计目的** |
| 时钟漂移反复触发 | 先修环境, 不要在漂移状态下继续下单 |

> **紧急按住全部**(057 新增): 多个实例同时异常时用 `ricow kill`(daemon 继续活着)或 Web 的
> `POST /api/runs/kill` —— 与 `stop --close-all` 分工不同: 后者平**单个**实例的仓, 前者按**全部**实例。
> 平仓是不可逆动作, 必须显式加 `--close-all`; 两条渠道的确认都是**逐字短语**(`确认停止全部` /
> `确认停止全部并平仓`), 裸 `y` 不认。`stopped == 0` 时**不会**套"已全部停止"头衔, 只说当前没有运行中的实例。

**回滚**: `ricow stop <name> --close-all` → `ricow info <name>` 确认清零 → 交易所端复查资金 → 需要时**轮换 API key** → 把 `live_enabled` 改回 `false`。

---

## 七、常见坑(都是本项目实测踩过的)

1. **时钟**: 币安对超前 >1s 的签名请求直接拒(本机曾漂到 +1120ms); 合约与现货服务器时间**不同步**, 按市场分别取数。
2. **订单号**: 交易所限制 `clientOrderId` ≤ 36 字符(平仓单一度 43 字符被拒); 引擎已截断, 但自定义策略别再拼接长串。
3. **最小名义额**: 现货 `minNotional` 5 USDT; 卖出不足最小额会拒 → 会留下**卖不掉的尘埃**(实测残留过 0.000084 ETH), 属正常。
4. **用户流**: 现货用户流走官方 WebSocket API(`userDataStream.subscribe.signature`, legacy listenKey 已于 2026-02 下线); 合约走 fapi listenKey。
5. **成交回写**: 实盘成交只能经用户流送达, 引擎在停机清理后会短暂吸干用户流(≤5s)以捞回兜底平仓的成交 —— 若关了它, 报表会漏最后几笔。
6. **数据目录**: 别把 `RICOW_ROOT` 放在网络盘/云同步盘(SQLite WAL 不支持网络文件系统)。
7. **资金费与强平**(合约): 资金费以交易所账单为准(`/fapi/v1/income`), 每 8h 结算; 距强平低于 `params.liq_warn_pct`(默认 15%)会告警 —— **只提示不动作**。
8. **门禁别绕过就走**: `live_enabled` + `--live` 双条件、Dry Run 时长、时钟预检 —— 每一道拒绝都带着解除方式, 先读它。

---

## 七之二、整理过程中发现并处理的问题

1. ~~**Dry Run 虚拟本金不可配(100,000 硬编码)**~~ → ✅ **已修: 016-dryrun-initial-cash**(`params.initial_cash`):
   同一份配置现在能同时预演 Dry Run 与实盘; 实测 `initial_cash = 300.0` 下建仓 `150 USDT`、拒单 0
   (对比修复前同配置 `65 单全被拒`)。非法值(≤0/NaN/inf)会报错而非静默回落到 100k。
2. ~~**`[risk]` 字段类型不统一**(名义/亏损是小数, 滑点/频率/周期数是整数)~~ → ⛔ **整段已作废(2026-10-18)**:
   `[risk]` 段连同 `RiskEngine` 四条静态限额已于 019-R5 删除, 现在**照着写也不报错、也不生效**。
   单笔最小名义改用策略参数 `min_notional`(真生效)。详见 §一「关于 `[risk]` 段」。

---

## 七之三、当前阶段怎么推进(实盘暂时没有资金)

按现状把清单**拆成两半用**, 不必等资金到位:

| 想验的东西 | 怎么验 | 真实度 |
|:--|:--|:--|
| 下单/撤单/成交回写/风控/停机清理等**交易链路** | 币安 **demo 测试网**(`RICOW_BN_BASE_URL=https://demo-api.binance.com`、`RICOW_FAPI_BASE_URL=https://demo-fapi.binance.com`, 凭据见 `specs/testnet.md`)—— 真实 API 往返、真实撮合语义, 无真实资金 | 高(无真实资金风险) |
| 策略在**真实行情**下的行为、参数是否顺手、通知是否可达 | **主网 Dry Run**: 不设 demo 域名(默认连主网公共行情), `live_enabled = false`, 用 `initial_cash` 设成真实资金口径 | 行情真实、成交虚拟 |
| 真实资金下的滑点/成交/资金费/强平 | **必须等有资金**: 阶段 3(`--live` + `live_enabled=true`), 且先满足 Dry Run 时长门禁 | — |
| 回测口径本身 | 阶段 0 基线; 与 Dry Run 手工核对 | 高 |

也就是说: **阶段 0/1/2 与"主网 Dry Run 预演"现在就能做**, 只有"阶段 3 实盘首日"需要等资金;
届时前面积累的 Dry Run 时长会自动满足 002 的门禁(默认 24h), 不必再等一轮。

## 七之四、两条来自真实预演的教训(017)

1. **单笔最小名义必须显著低于"最小再平衡量级"**, 否则策略每个 tick 都会被拒一次:
   实测(2026-09, 当时配的是 `[risk].min_order_notional`)把该值设成 20 USDT, 而再平衡残差只有 ~19.6 USDT
   → 一轮 110 秒里 **209 次拒单**、日志刷屏。判据: 该值应 ≤ 单笔再平衡量的 1/3(网格单笔约 25 USDT 时, 设 5~8 即可)。
   拒单持续增长是"参数不匹配"的信号, 不是策略坏了。
   > ⚠️ **2026-10-18 订正**: 上面那次实测用的是 `[risk].min_order_notional`, 该配置面**已被019-R5 删除**。
   > 同一判据现在对应策略参数 **`min_notional`**(`strategies/spot/shannon_grid.toml`, 默认 5, **真生效**)。
   > 结论的**形状**仍成立(值太大 → 每个 tick 拒一次), 只是配置点换了。
2. **WSL 时钟会跳变, 抓窗口要"检查→立刻开跑"**: 实测同一分钟内可从 -2847ms 跳到 +996ms, 导致预检/签名被拒
   (`Timestamp for this request is outside of the recvWindow`)。可复用的抓窗口脚本:

```bash
for i in $(seq 1 40); do
  SKEW=$(python3 -c "import json,time,urllib.request as u;print(int(time.time()*1000)-json.load(u.urlopen('https://api.binance.com/api/v3/time',timeout=8))['serverTime'])")
  [ "$SKEW" -lt -1200 ] && break   # 滞后窗口 → 立刻开跑
  sleep 1
done
ricow start <name> --live
```
   稳定方案仍是修时钟(Windows 侧 `wsl --shutdown` 重开, 或 `sudo hwclock -s`)。

## 八、结果留档(执行时填)

| 阶段 | 日期 | 关键数字 | 结论 |
|:--|:--|:--|:--|
| 0 回测基线 |  | 成交 __ 笔 / 手续费占比 __% / 回撤 __% / 拒单 __ |  |
| 2 Dry Run(≥24h) |  | tick __ / 成交 __ / 拒单 __ / 异常日志 __ |  |
| 3 实盘首日 |  | 成交 __ 笔 / 手续费 __ / 拒单 __ / 熔断 __ |  |
| 3 实盘累计(约 1 周) |  | 净盈亏 __ / 与回测偏差 __ / 账目核对 ✅❌ |  |
| 收尾 |  | 残留挂单 __ / 残留持仓 __ / key 是否轮换 |  |

**P4 通过判据(回到发布门槛)**: 连续运行无"莫名亏损"(每一笔亏损都能被行情或策略规则解释)、
账目与交易所一致、护栏与通知在真实资金下被触发过并不误伤 → 方可进入公开发布准备。

---

## 八之订正(2026-10-18, 对照最新代码逐条核对)

本轮审核发现本手册有**三处照着做会直接失败**的地方, 已全部订正:

| # | 旧文档写的 | 实际 | 后果 |
|:--|:--|:--|:--|
| 1 | `ricow keyring set binance_key --key <file>` | **该命令不存在**(与 OS Keyring / `credentials.toml` 一起于 2026-09-14 移除); 唯一入口是 `ricow.toml` 的 `[exchange]` 段(`binance_key`/`binance_secret`/`demo_key`/`demo_secret`)或 env | 照抄 → 命令未找到, 凭据没入库 |
| 2 | 路径 A 的 TOML 模板含 `order_size` / `target_ratio` / `rebalance_band`; `[risk]` 段四字段 | `shannon_grid` 清单里**没有这三个参数**; `[risk]` 段已于 019-R5 删除、**静默忽略不报错** | 照抄 → 策略参数全是空转; `[risk]` 段给出"设了护栏"的假象 |
| 3 | `ricow backtest --strategy shannon_grid --pair ETHUSDT --days 30` | `start_price` 是 `required = true` 且**无默认值** | 照抄 → `[FATAL] 缺少必填参数 start_price`, 0 成交 |

另外三处口径补齐: ① `start_price` / `invest_cash` 等**必填参数**的给法(阶段 0 命令已补`--param`)、
② 部署物落盘路径是 `strategies/spot/<id>.lua`(**没有 `scripts/` 子目录**)、③ 平台侧护栏的真实形态
(频率 + 单笔名义两条, 无最大持仓/日亏/滑点/回撤熔断), 并补上 057 的 `ricow kill` 作为多实例异常时的
紧急按住手段。

**仍未变的部分**: 时钟预检、`min_notional` 量级判据、`min_dry_run_hours` 双条件门禁、
`live_enabled` + `--live`、Dry Run 每次从零开始、成交回写只能经用户流 —— 这些经代码核对**依然成立**。
