# 065 收敛记录 —— 回测网络失败要给可操作的指引

## 1. 问题（一手证据）

用户在 Web 上点「模拟回测」，作业失败，界面上只有一行裸错误：

```
network error: K 线第 0 片拉取失败: network error: error sending request for url
(https://api.binance.com/api/v3/klines?symbol=ETHUSDT&interval=1h&limit=1000)
```

**这句话对用户没有任何帮助** —— 他不知道该查网络、查代理，还是自己代码写错了。

### 根因不是产品缺陷（先证伪环境）

本机实测（直连与经代理两条都试）：

| 目标 | 直连 | 经代理 |
| -- | -- | -- |
| `api.binance.com` | HTTP 000 | **502 Bad Gateway**（代理侧屏蔽） |
| `data-api.binance.vision` | **HTTP 200** | **HTTP 200**（币安官方公共行情镜像） |
| `testnet.binancefuture.com` | — | **HTTP 200** |
| `fapi.binance.com` | HTTP 000 | HTTP 000 |
| `github.com` / `google.com` | HTTP 000 | HTTP 000（本机整体网络不通，代理只放行部分站点） |

另：系统 DNS 把 `api.binance.com` 解析到**随机 IP**（两次查询分别是 `23.225.141.210` / `157.240.18.18` —— 典型的 DNS 污染）。

**实证**：用 `RICOW_BN_BASE_URL=https://data-api.binance.vision` +
`RICOW_FAPI_BASE_URL=https://testnet.binancefuture.com` 起服务后重跑 e2e → **460 PASS / 0 FAIL**
（此前 451/18，18 条全在回测段）。

⇒ 那 18 条红**不是产品缺陷**，是币安主网在本机不可达。这条从"假定是环境问题"变成了**实跑证明**。

### 但"环境问题"不等于"用户不用管"

产品**早就知道**这件事 —— `crates/ricow/src/ai/prompt.rs` 第 8 条写着：

> 网络: 全程需访问币安。国内直连 api.binance.com 常超时, CLI 认 HTTPS_PROXY/HTTP_PROXY/ALL_PROXY;
> RICOW_BN_BASE_URL/RICOW_FAPI_BASE_URL 是整体域名替换(公开数据+签名下单都变),
> 公开数据镜像域(如 data-api.binance.vision)只能看行情/回测, 下单会失败。

**但这段话是写给 AI 看的，用户看不到。** Web 界面上只有那行裸错误。

## 2. 做了什么

回测/寻优/多维优化/验证**四个作业级 error 渲染点**，在错误原文下面追加一条指引块：

| 元素 | 内容 |
| -- | -- |
| 标题 | 「这是网络/代理问题, 不是策略写错了。」 |
| 动作 1 | 检查代理：`HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` |
| 动作 2 | 或用备用数据域名：`RICOW_BN_BASE_URL=https://data-api.binance.vision`（**只影响行情与回测，下单仍走主网**） |
| 动作 3 | 合约同理：`RICOW_FAPI_BASE_URL=https://testnet.binancefuture.com` |

### 判据要**窄**

只认 `network error` / `拉取失败` 两个形态。参数错、门禁拦、Lua 语法错**都不提示** ——
提示了是噪音，且会把人引向错误的排查方向（"你的参数错了" vs "网络不通"）。

### 措辞纪律

1. **明说"不是策略写错了"** —— 用户第一反应是怀疑自己。
2. **明说备用域名只影响行情与回测、下单仍走主网** —— 不说这句，用户会拿它去实盘，
   然后被签名接口拒掉，比原来更困惑。

## 3. 验证

| 项 | 结果 |
| -- | -- |
| 回归断言 `test_backtest_network_error_shows_actionable_guidance` | 通过 |
| **变异验证 5/5 全抓住** | M1 删挂载点 / M2 放宽判据 / M3 指引块不渲染 / M4 去掉"下单仍走主网" / M5 换掉备用域名 |
| 065 专项走查 `probe065.mjs` | **12 PASS / 0 FAIL**（网络错误→指引出现；参数错/门禁错/Lua 错→正确地**不**出现） |

### 变异验证抓到的两个真问题

1. **M4 第一版漏了**：断言写成 `contains("中文那句") || contains("英文那句")` ——
   变异掉中文那份时英文还在 ⇒ 仍绿。**"或"让断言失去牙齿**。
   修：两份文案**分别**断言（"与"不是"或"）—— 中英是两份独立文案，少一份就少一份限定。
2. 断言里 `assert_eq!(bt.matches("appendNetGuidance(box, job.error)").count(), 4)` ——
   钉住**四个**作业级渲染点都挂了（漏一个就有一个入口看不到指引）。

## 4. 刻意不做

| 项 | 理由 |
| -- | -- |
| 在 `ricow.toml` 里加 `base_url` 配置项 | 那是**新增配置面**，要过 `WRITABLE` 与安全审查。本轮只把**已有的 env 覆盖口**告诉用户 |
| 自动探测并自动切换镜像 | 自动改数据源会让用户不知道自己在用哪个域；且镜像只覆盖公开数据，自动切过去后下单会失败 —— 那比报错更坏 |
| 给所有错误加提示 | 只有网络类错误给。参数/门禁错误提示网络方向是误导 |
| 前端改 DNS / 改代理 | 前端不能也不该做系统级网络配置 |
