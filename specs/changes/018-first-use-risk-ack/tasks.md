# 018 任务分解

> 状态: **全部完成** (2026-09-13) | 基线: 306 → **308 passed / 0 failed / 11 ignored**(零警告)

- [x] **T001** `RISK_DISCLOSURE` 披露要点(四条: 仅供学习/无止损与选品责任/先 Dry Run + 子账号小额/不代管资金密钥)
- [x] **T002** `RiskGate`(Proceed / JustAcked / Refuse{message}) + `risk_gate()` 纯函数
- [x] **T003** 单测 2 例: 首次拒绝(消息含披露/开关/README); 开关放行与已确认放行
- [x] **T004** CLI: `risk_ack_path/risk_acked/write_risk_ack`(版本化 JSON)
- [x] **T005** `run --accept-risk` + 最前置判定; `start --accept-risk` + 前置判定(父进程落地记录供子进程放行)
- [x] **T006** 真实验证(全新数据目录 + demo): ① 不带开关 → 拒绝且**无** `risk_ack.json`; ② `--accept-risk` → 记录落地(版本/时间戳/披露正文); ③ 不带开关再跑 → 不再被拦, 直接进时钟预检并启动
- [x] **T007** demo 账户核对: 无遗留挂单/持仓(仅尘埃)
- [x] **T008** 文档同步 + `converge.md`

## 实施记录 (2026-09-13)

- 加 `--accept-risk` 字段后, `ctrl.rs` 内部测试构造 `StartArgs` 缺字段导致一次编译失败(E0063) → 补 `accept_risk: false`;
  教训: 给 clap 结构体加字段时, 记得搜一遍**测试里的字面构造**。
- 验证③(已确认后不带开关再跑)顺带证明了判定顺序正确: 风险闸放行后直接落到时钟预检并成功启动。
