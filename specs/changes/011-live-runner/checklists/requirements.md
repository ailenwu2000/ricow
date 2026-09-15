# 规格质量检查清单: 实盘运行器 (live_enabled 生效)

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-09-13
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs) — 仅以"代码事实"引用既有缺口位置作为依据, 未规定实现方式
- [x] Focused on user value and business needs (P4 dogfood 入口 / 不留残单裸仓 / 实盘可见)
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [待澄清] markers remain — 已全部转为 §口径拍板 默认值(4 项); 复核确认后关闭
- [x] Requirements are testable and unambiguous (按 §口径拍板 取值后均无歧义)
- [x] Success criteria are measurable (SC-001~005 均可复现)
- [x] Success criteria are technology-agnostic
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified (时钟/对齐/归属/初始仓/部分成交/重复停机/断线/拒单)
- [x] Scope is clearly bounded (不做项显式列出: 钱包模型校准、003、002、主网 dogfood)
- [x] Dependencies and assumptions identified (A1~A10)

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification — 仅以"起点事实"引用既有缺口位置作为依据, 未规定实现方式

## Notes

- Items marked incomplete require spec updates before `/speckit-clarify` or `/speckit-plan`
- 用户本轮未在确认表单作答(超时): 4 项口径已按"最简 + 有代码依据"取默认并写入 spec §口径拍板(含依据与推翻成本), FR-010/FR-013/FR-015 与假设 A1~A3 已按默认回填 → **spec 无待澄清残留, 可直接进 plan/implement**
- 其中"拍板 2 市场范围"是本轮**唯一由代码事实改写**的默认: 复核发现 `Exchange` 仅现货有实现、合约缺 fapi WS → 合约实盘降级为 Phase 2(可拆 012); 若用户要求合约同批, 需先立适配层任务
