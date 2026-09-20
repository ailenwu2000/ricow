# 快速验证: 027 停机动作对"不存在的策略名"误报成功

**规格**: [spec.md](./spec.md) | **计划**: [plan.md](./plan.md) | **契约**: [contracts/cli-stop.md](./contracts/cli-stop.md) · [contracts/control-channel.md](./contracts/control-channel.md) | **日期**: 2026-09-20

四类名字 × 两条通道(终端 CLI / 对话内 AI), 覆盖 FR-002~FR-014 与 SC-001~SC-006。

## 0. 前置

```powershell
# 构建 (项目根)
cargo build -p ricow

# 隔离数据根, 避免动到正在用的实例与台账
$env:RICOW_ROOT = "$env:TEMP\ricow-027-qs"
New-Item -ItemType Directory -Force "$env:RICOW_ROOT\strategies" | Out-Null
New-Item -ItemType Directory -Force "$env:RICOW_ROOT\run" | Out-Null

# 起一个独立 daemon (这条通道是停机的前置)
.\target\debug\ricow.exe daemon start
.\target\debug\ricow.exe list          # 确认 daemon 在线 (无 "daemon 未运行" 提示)
```

## 1. 准备四类夹具

```powershell
$EXE = ".\target\debug\ricow.exe"

# B 类: 半存在 (只有 TOML, 从未启动) —— 复制一份真实策略改名即可
Copy-Item "$env:RICOW_ROOT\strategies\<某个已部署名>.toml" "$env:RICOW_ROOT\strategies\only-toml.toml"

# C 类: 存在但未运行 (跑过又停了) —— 启动后停掉, 留下台账
$EXE start seeded-then-stopped
$EXE stop seeded-then-stopped

# D 类: 运行中 —— 用测试网(demo)真实实例, 不涉真实资金
$EXE start demo-target --demo

# A 类: 不存在 —— 不需要任何准备
$NAME_A = "no-such-strategy"
```

## 2. 场景 A: 名字不存在 (P1 / 用户故事 1)

```powershell
$EXE stop $NAME_A;               "exit=$LASTEXITCODE"
$EXE stop $NAME_A --close-all;   "exit=$LASTEXITCODE"
$EXE restart $NAME_A;            "exit=$LASTEXITCODE"
$EXE list                        # 报错文案指向的下一步命令
```

**期望**: 三条命令 `exit ≠ 0`(实测 1); stderr 含 `错误: invalid argument: 未找到策略或实例 no-such-strategy (strategies/no-such-strategy.toml 不存在, 也无实例台账); 可用 ricow list 查看现有实例`; **不出现** `已停止` / `未在运行` / `无需停止`; `restart` 也无任何"已停止"式回执, 且事后 `list` 里没有该名字。

对照 [contracts/cli-stop.md §五](./contracts/cli-stop.md#五验收对照-人工逐条比对用) 的 C1/C2/C3。

## 3. 场景 B/C: 存在但未运行 (P2 / 用户故事 2)

```powershell
$EXE stop only-toml;                 "exit=$LASTEXITCODE"   # 半存在
$EXE stop seeded-then-stopped;       "exit=$LASTEXITCODE"   # 跑过又停了
```

**期望**: 两条均 `exit = 0`, stdout **只有一行** `该策略未在运行 (无需停止)`, **不含** `已停止`。

> `only-toml` 这一条同时验证 spec §三 边界情况里"仅有 TOML 从未启动 → 判为存在"的拍板(D1)。

## 4. 场景 D: 运行中停机 (零回归, SC-005)

```powershell
$EXE stop demo-target;                    # 与改动前逐字比对
$EXE start demo-target-2 --demo
$EXE stop demo-target-2 --close-all       # 与改动前逐字比对
```

**期望**: 输出与退出码与**改动前完全一致** —— 含 `测试网停机(模拟盘, 与真实资金无关)` 与 `--close-all` 的 `平掉策略持仓` 说明。此处**不允许**有任何新措辞。

## 5. 场景 E: daemon 未运行的优先级 (FR-009)

```powershell
$EXE daemon stop          # 或在另一终端停掉
$EXE stop no-such-strategy;   "exit=$LASTEXITCODE"
$EXE stop only-toml;          "exit=$LASTEXITCODE"
$EXE daemon start         # 恢复
```

**期望**: 两条都先报 `错误: invalid argument: daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start`, **不得**改口成 `策略未在运行` / `无需停止`。

## 6. 场景 F: 空串 / 纯空白 (FR-008)

```powershell
$EXE stop "";      "exit=$LASTEXITCODE"
$EXE stop "   ";   "exit=$LASTEXITCODE"
```

**期望**: 两条均非 0, 落入"不存在"分支报错; **不得**落入"未在运行 (无需停止)"。

## 7. 场景 G: 口径统一 (P3 / 用户故事 3)

在同一 `RICOW_ROOT` 下, 对上面四类名字分别走**两条通道**:

```powershell
# 终端通道
$EXE stop no-such-strategy
$EXE stop only-toml

# 对话通道
$EXE ai --plain
# 会话内依次说:
#   "停掉 no-such-strategy"
#   "停掉 only-toml"
#   "停掉 demo-target"
```

**期望**: 对同一名字, 两条通道结论逐条一致 —— 都不存在 / 都未在运行 / 都可停机; **不出现**一边报错一边成功。AI 侧不存在分支指向 `instance_status`, CLI 侧指向 `ricow list`(同一判定、同一文案基底, 仅"下一步命令"的名字不同)。

## 8. 不回归比对 (FR-017)

```powershell
$EXE status      ; $EXE status no-such-strategy
$EXE info no-such-strategy
$EXE logs no-such-strategy
$EXE list
$EXE fills no-such-strategy
```

**期望**: 与改动前**逐字一致**(见 [contracts/cli-stop.md §四](./contracts/cli-stop.md#四明确不变面-fr-017--d11))。

## 9. 自动化

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p ricow --bin ricow
```

**期望**: fmt 零差异; clippy 零警告; 测试全绿且 `failed = 0`, passed 数相对改动前基线**不倒退**。对应单测/集成测见 [contracts/control-channel.md §六](./contracts/control-channel.md#六验收断言-单测集成测)(P1~P5)与 [plan.md §一 D10](../plan.md)。

## 10. 收尾

```powershell
$EXE stop demo-target --close-all      # 若有残留实例
$EXE daemon stop
Remove-Item -Recurse -Force $env:RICOW_ROOT
Remove-Item Env:\RICOW_ROOT
```

> 按宪法行 42, 验收过程**不做** `git commit`; 需要提交时由用户明确指示。
