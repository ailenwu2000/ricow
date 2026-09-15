# P4 dogfood 执行清单 (真实小资金实盘闭环)

**版本**: 2026-09-13 | **适用**: ricow 011(Dry Run/实盘运行器)+ 002(建策略入口与 Dry Run 时长门禁)+ 003(出站通知)+ 013/014(合约记账与资金费/强平可见)
**门槛来源**: `specs/roadmap.md` P4 =「dogfood 实盘 → 公开发布」, 发布判据 = **"可用且不会让用户莫名亏损"**。

---

## 零、纪律边界(先读, 别跳)

1. **仓库的测试纪律不变**: 宪法/`product.md` §十一 规定交易流程测试必须走币安 demo 测试网, **禁主网下单测试**。
   本清单是**产品级 dogfood(由用户本人执行的真实小资金运行)**, 不是 repo 测试 —— 因此它**不进 `cargo test`**,
   也不得因此往仓库里加"主网集成测试"。
2. **部署/系统级操作与资金操作由你本人执行**; 我(agent)只给步骤、命令与判据, 不代跑、不代持凭据。
3. **只用亏得起的钱**, 并把策略限制在**交易所单独子账号**里(product.md §十二 建议): 网格会吃掉账号内大部分现金,
   子账号隔离风险且独立核算。

---

## 一、一次性准备

| # | 事项 | 命令/做法 | 判据 |
|:--|:--|:--|:--|
| 1 | 交易所子账号 | 交易所后台建子账号, 转入**小资金** | 建议 **200~500 USDT**(下面示例按 300 设计) |
| 2 | API key 权限 | **只开"现货交易"**(合约另开"合约交易"); **关闭提现**; 建议绑 IP | 权限页截图留档 |
| 3 | 凭据入库(**只有阶段 3 实盘需要**) | `ricow keyring set binance_key --key <file>` + `ricow keyring set binance_secret --key <file>` | `ricow keyring list`; **或**用 env `RICOW_BN_API_KEY`/`RICOW_BN_SECRET_KEY`。<br>⚠️ **主网 Dry Run 与回测都不需要凭据**(只读公共行情); demo 真实调用需要 demo 专属凭据(见 `specs/testnet.md`); 实盘才需要主网凭据 |
| 4 | 时钟预检 | 见下方命令 | 本机超前 ≤1000ms、滞后 ≤4000ms(超了会被硬拒) |
| 5 | 出站通知(可选, 强烈建议) | 策略 TOML 里 `params.notify_webhook = "https://api.telegram.org/bot<TOKEN>/sendMessage"` + `notify_chat_id` | 实盘时熔断/成交/接近强平/停机残留会推到手机 |
| 6 | 数据目录 | `export RICOW_ROOT=<独立目录>`(避免与 demo 探针数据混在一起) | 该目录下会生成 `ricow.db` / `strategies/` / `logs/` |

> ⚠️ `[risk]` 各字段类型不统一: `max_position_notional`/`max_daily_loss_usd`/`min_order_notional` 是**小数**,
> `max_slippage_bps`/`max_orders_per_sec`/`consecutive_loss_periods` 是**整数** —— 写错类型 TOML 解析会直接失败
> (本清单模板已按真实类型写好, 照抄即可)。

时钟检查(WSL 漂移是常见坑, 实测出现过 +1120ms 被拒):

```bash
python3 -c "import time,json,urllib.request as u;print(int(time.time()*1000)-json.load(u.urlopen('https://api.binance.com/api/v3/time'))['serverTime'],'ms')"
# 绝对值持续 >500ms 就先修: Windows 侧 `wsl --shutdown` 重开, 或 sudo hwclock -s
```

---

## 二、阶段 0 — 回测基线(交易所无关, 先跑, 留档对照)

```bash
ricow backtest --strategy shannon_grid --pair ETHUSDT --days 30
```

**看什么**: `成交笔数` / `手续费占比`(现货网格实测约 **0.10%**/笔 —— 对应 taker 10bps)/ `最大回撤` / `拒单次数`(应为 0) / `净盈亏`。
**为什么**: 实盘跑起来后要拿真实成交的手续费与笔数**和这份基线对比** —— 差得多就是记账或撮合口径有问题(013/014/015 修的就是这类)。
> 注意: 回测窗口随当前时间滑动, **不同时刻跑出的数字不可直接比较**; 比较必须同一时刻连续跑两次或留档同一次的输出。

---

## 三、阶段 1 — 建立待跑策略(两条等价路径)

**路径 A(手写, 推荐首跑用)**: 写 `$RICOW_ROOT/strategies/shannon-dogfood.toml`。

> ✅ **一份配置即可预演**: Dry Run 的虚拟本金可配(`params.initial_cash`, 016 落地, 缺省 100000)。
> 把它设成**与实盘相同的资金口径**, 同一套 `[risk]` 限额就能同时适配 Dry Run 与实盘 —— 不必再为两个口径维护两套配置。
> (016 之前 Dry Run 本金硬编码 100k: 实测用小资金限额跑 Dry Run 会 `65 单全被拒`。)

```toml
[strategy]
name = "shannon-dogfood"
type = "shannon_grid"        # 内置网格策略; 或 type = "lua" + params.script_path
enabled = true
exchange = "binance"
live_enabled = false         # 先别开; 阶段 3 再打开
market = "spot"

[strategy.params]
pair = "ETHUSDT"
initial_cash = 300.0         # Dry Run 虚拟本金 = 真实小资金口径(缺省 100000); 启动会打印它以便核对
order_size = 0.01            # 单笔约 25 USDT
target_ratio = 0.5
rebalance_band = 0.005
atr_period = 14
atr_mult = 1.0
# notify_webhook = "https://api.telegram.org/bot<TOKEN>/sendMessage"
# notify_chat_id = "<CHAT_ID>"

[risk]                       # 按 300 USDT 本金设; Dry Run 与实盘共用这一套
max_position_notional = 300.0   # = 本金, 不允许加仓超过本金
max_daily_loss_usd = 15.0       # 本金 5%: 到了就停新开仓
min_order_notional = 20.0       # 现货 minNotional 5 USDT; 20 可挡碎单
max_slippage_bps = 20           # 整数(bps)! 写成 20.0 会 TOML 解析失败
```

**三种口径的资金来源(容易混, 单独说清)**:

| 模式 | 资金/权益从哪来 | `[risk]` 限额该按什么设 |
|:--|:--|:--|
| 回测 | CLI `--cash`(缺省 100000) | 按回测假设的本金 |
| **Dry Run** | `params.initial_cash`(缺省 100000; 016 可配) | 按 `initial_cash` —— 设成与实盘同一口径即可复用同一套限额 |
| **实盘 / demo** | **交易所账户权益**(与 `initial_cash` 无关) | 按**该账户实际权益**设 —— 在 demo 账户(~5000 USDT)上跑"300 USDT 小资金"配置会被限额全拒 |

> `[risk]` 字段类型不统一: `max_position_notional`/`max_daily_loss_usd`/`min_order_notional` 是**小数**,
> `max_slippage_bps`/`max_orders_per_sec`/`consecutive_loss_periods` 是**整数** —— 写错类型只会得到
> `TOML parse error`(不告诉你是哪个字段)。照抄上面模板即可。

**路径 B(AI/提交式)**: `ricow create --name shannon-dogfood --pair ETHUSDT --param order_size=0.01 --script <file.lua>`
→ `ricow approve <preview_id>` → `ricow deploy <preview_id> --token <t>`。部署物同样是 `strategies/<name>.toml` + `<name>.lua`。

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
- `ricow logs shannon-dogfood --lines 50` —— 有无 `WARN`/`ERROR`; 有无 `风控拒单`
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
- 有没有 `风控拒单` / `Rejected`(预期: 0 —— 拒单说明限额设得太紧或策略在乱下单)
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
| 出现一级/二级熔断通知 | 策略在持续亏损 —— 先看是行情还是 bug |
| 拒单数持续增长 | 风控阈值过紧 / 策略在反复下无效单(如零动作单) |
| 账目对不上(交易所成交 ≠ `ricow fills`) | 成交回写或记账缺陷 —— 停止并留存日志 |
| 单日亏损超过 `max_daily_loss_usd` | 护栏应已拦住新开仓; 确认它真的拦住了 |
| 时钟漂移反复触发 | 先修环境, 不要在漂移状态下继续下单 |

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
2. **`[risk]` 字段类型不统一**(名义/亏损是小数, 滑点/频率/周期数是整数) —— 写错类型只会得到
   `TOML parse error`, 不会告诉你哪个字段该是什么类型。本清单已按真实类型写好, 建议后续把这条写进
   `specs/lua-api.md` 的配置表(或让报错带上期望类型)。

---

## 七之三、当前阶段怎么推进(实盘暂时没有资金)

按现状把清单**拆成两半用**, 不必等资金到位:

| 想验的东西 | 怎么验 | 真实度 |
|:--|:--|:--|
| 下单/撤单/成交回写/风控/停机清理等**交易链路** | 币安 **demo 测试网**(`RICOW_BN_BASE_URL=https://demo-api.binance.com`、`RICOW_FAPI_BASE_URL=https://demo-fapi.binance.com`, 凭据见 `specs/testnet.md`)—— 真实 API 往返、真实撮合语义, 无真实资金 | 高(无真实资金风险) |
| 策略在**真实行情**下的行为、参数与 `[risk]` 限额是否顺手、通知是否可达 | **主网 Dry Run**: 不设 demo 域名(默认连主网公共行情), `live_enabled = false`, 用 `initial_cash` 设成真实资金口径 | 行情真实、成交虚拟 |
| 真实资金下的滑点/成交/资金费/强平 | **必须等有资金**: 阶段 3(`--live` + `live_enabled=true`), 且先满足 Dry Run 时长门禁 | — |
| 回测口径本身 | 阶段 0 基线; 与 Dry Run 手工核对 | 高 |

也就是说: **阶段 0/1/2 与"主网 Dry Run 预演"现在就能做**, 只有"阶段 3 实盘首日"需要等资金;
届时前面积累的 Dry Run 时长会自动满足 002 的门禁(默认 24h), 不必再等一轮。

## 七之四、两条来自真实预演的教训(017)

1. **`min_order_notional` 必须显著低于"最小再平衡量级"**, 否则策略每个 tick 都会被风控拒一次:
   实测把 `min_order_notional` 设成 20 USDT, 而再平衡残差只有 ~19.6 USDT → 一轮 110 秒里 **209 次拒单**、日志刷屏。
   判据: 该值应 ≤ 单笔再平衡量的 1/3(网格单笔约 25 USDT 时, 设 5~8 即可)。拒单持续增长是"参数不匹配"的信号, 不是策略坏了。
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
账目与交易所一致、风控与通知在真实资金下被触发过并不误伤 → 方可进入公开发布准备。
