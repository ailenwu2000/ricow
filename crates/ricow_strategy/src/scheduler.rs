//! 策略调度器: 管理多个策略实例, 驱动 tick 循环。

use std::collections::HashMap;

use crate::context::Context;
use crate::risk::RiskEngine;
use crate::strategy::Strategy;

/// 单个策略实例 (策略 + 运行上下文)。
struct StrategyInstance {
    strategy: Box<dyn Strategy>,
    context: Box<dyn Context>,
}

/// 策略调度器。
pub struct StrategyScheduler {
    instances: HashMap<String, StrategyInstance>,
    risk: RiskEngine,
}

impl StrategyScheduler {
    pub fn new() -> Self {
        Self { instances: HashMap::new(), risk: RiskEngine::new() }
    }

    pub fn with_risk(risk: RiskEngine) -> Self {
        Self { instances: HashMap::new(), risk }
    }

    /// 添加策略实例。
    pub fn add_strategy(
        &mut self,
        id: impl Into<String>,
        strategy: Box<dyn Strategy>,
        context: Box<dyn Context>,
    ) {
        self.instances.insert(id.into(), StrategyInstance { strategy, context });
    }

    pub fn remove_strategy(&mut self, id: &str) -> bool {
        self.instances.remove(id).is_some()
    }

    pub fn has(&self, id: &str) -> bool {
        self.instances.contains_key(id)
    }

    pub fn strategy_count(&self) -> usize {
        self.instances.len()
    }

    pub fn ids(&self) -> Vec<String> {
        self.instances.keys().cloned().collect()
    }

    /// tick 所有策略一次: on_tick → 风控 → 下单 → drain fills → on_fill。
    pub fn tick(&mut self) {
        let ids: Vec<String> = self.instances.keys().cloned().collect();
        for id in ids {
            self.tick_one(&id);
        }
    }

    fn tick_one(&mut self, id: &str) {
        let Some(inst) = self.instances.get_mut(id) else { return };
        let StrategyInstance { strategy, context } = inst;

        let orders = strategy.on_tick(&mut **context);
        for req in orders {
            if let Err(e) = self.risk.check(&req, &**context) {
                context.log(&format!("风控拒绝: {e}"));
                continue;
            }
            if let Err(e) = context.place_order(req) {
                context.log(&format!("下单失败: {e}"));
            }
        }
        let fills = context.drain_fills();
        for fill in fills {
            strategy.on_fill(&mut **context, fill);
        }
    }

    /// 停止所有策略。
    pub fn stop_all(&mut self) {
        for inst in self.instances.values_mut() {
            inst.strategy.on_stop(&mut *inst.context);
        }
        self.instances.clear();
    }
}

impl Default for StrategyScheduler {
    fn default() -> Self {
        Self::new()
    }
}
