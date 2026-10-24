// Ricow Web 前端「策略」视图 — 回测与寻优 (042 从 strategies.js 拆出):
//   发起回测 / 1s 三态轮询 / 指标卡 / 权益曲线(基准线 + 逐点回撤) / 平仓盈亏明细 /
//   策略日志块 / 参数寻优对比表 / 会话级回测历史 / 结论带回 AI。
// 取用: 工具与共享状态来自 `R.sg`; 跨文件符号见本文件末尾的 `S.xxx =`。
// 本文件**不做**: 列表、详情表单、Lua 编辑器(各归其文件)。
"use strict";

(function () {
  const R = window.Ricow;
  const S = R.sg;
  const t = S.t;
  const $ = S.$;
  const h = S.h;
  const setMsg = S.setMsg;
  const st = S.st;
  const POLL_MS = S.POLL_MS;

  // 跨文件符号(042 D2: 延迟查找)
  const gatherParams = (...a) => S.gatherParams(...a);

  // ---------- 回测作业 ----------

  function syncBtBtn(busy) {
    const btn = $("sg-bt-go");
    btn.disabled = !!busy;
    btn.textContent = busy ? t("sgStartingBt") : t("sgStartBt");
  }

  /// 读高级项: 空 → 不带键; 非法(NaN/负)带字段名报错。
  function optionalNumber(id, key, positive) {
    const raw = $(id).value.trim();
    if (raw === "") return undefined;
    const n = Number(raw);
    if (!Number.isFinite(n) || (positive ? n <= 0 : n < 0)) {
      throw new Error(key + ": " + t("sgNumberInvalid"));
    }
    return n;
  }

  async function onBacktest() {
    if (!st.detailCtx) return;
    const strategy = st.detailCtx.id;
    const pair = $("sg-bt-pair").value.trim() || $("sg-pair").value.trim();
    const interval = $("sg-bt-interval").value;
    const daysRaw = $("sg-bt-days").value.trim();
    const days = Number(daysRaw);
    if (!pair) {
      setMsg($("sg-bt-msg"), t("sgPairRequired"), "err");
      return;
    }
    if (!Number.isInteger(days) || days < 1 || days > 3650) {
      setMsg($("sg-bt-msg"), t("sgDaysInvalid"), "err");
      return;
    }
    let body;
    try {
      // 回测参数宽松收集(空值跳过; 是否真能跑由作业 error 态如实承载)。
      const params = gatherParams(false);
      body = { strategy: strategy, pair: pair, interval: interval, days: days, params: params };
      const fee = optionalNumber("sg-bt-fee", t("sgFee"), false);
      if (fee !== undefined) body.fee = fee;
      const cash = optionalNumber("sg-bt-cash", t("sgCash"), false);
      if (cash !== undefined) body.cash = cash;
      const leverage = optionalNumber("sg-bt-leverage", t("sgLeverage"), true);
      if (leverage !== undefined) body.leverage = leverage;
    } catch (err) {
      setMsg($("sg-bt-msg"), err.message, "err");
      return;
    }

    const g = st.gen;
    syncBtBtn(true);
    setMsg($("sg-bt-msg"), "", "");
    try {
      const reply = await R.api("/api/backtest", { method: "POST", body: body });
      if (g !== st.gen) return;
      const job = {
        jobId: reply.job_id,
        status: "running",
        report: null,
        metrics: null,
        chart: null,
        logs: null,
        error: null,
        body: body,
        strategyLabel: strategy,
      };
      st.jobs.set(strategy, job);
      renderJob(job);
      startPolling(strategy, job.jobId, g);
    } catch (err) {
      if (g !== st.gen) return;
      // 同名作业在跑 → 409 busy; 若台账里正好有它, 直接把在途作业画出来。
      if (err && err.status === 409 && err.code === "busy") {
        const existing = st.jobs.get(strategy);
        if (existing && existing.status === "running") {
          renderJob(existing);
          startPolling(strategy, existing.jobId, g);
        } else {
          setMsg($("sg-bt-msg"), err.message || t("sgBusy"), "err");
        }
      } else {
        setMsg($("sg-bt-msg"), (err && err.message) || String(err), "err");
      }
    } finally {
      if (g === st.gen) syncBtBtn(false);
    }
  }

  /// 1s 轮询; 离开视图 / 切策略由 deactivate 或代际令牌停表, 绝不在旧视图上改 DOM。
  /// `strategy` 可以是策略 id(普通回测)或 id + 后缀(寻优作业: `#sweep` 单参数 / `#opt` 多维)。
  function startPolling(strategy, jobId, g) {
    if (st.pollTimer) clearInterval(st.pollTimer);
    const stillHere = () =>
      !!st.detailCtx &&
      (strategy === st.detailCtx.id ||
        strategy === st.detailCtx.id + "#sweep" ||
        strategy === st.detailCtx.id + "#opt" ||
        // 054 P4: 样本外检验作业(同一策略, 另一个作业槽 —— 它也可能跑很久)。
        strategy === st.detailCtx.id + "#val");
    const tick = async () => {
      let info;
      try {
        info = await R.api("/api/backtest/" + encodeURIComponent(jobId));
      } catch (err) {
        if (g !== st.gen || !stillHere()) return;
        // 404 = 作业过期被惰性清理; 其余网络抖动: 保留下一拍(运行态原样)。
        if (err && err.status === 404) {
          const j = st.jobs.get(strategy);
          if (j) {
            j.status = "error";
            j.error = t("sgBtExpired");
            renderJob(j);
          }
          if (st.pollTimer) clearInterval(st.pollTimer);
        }
        return;
      }
      if (g !== st.gen || !stillHere()) return;
      const job = st.jobs.get(strategy);
      if (!job || job.jobId !== jobId) {
        if (st.pollTimer) clearInterval(st.pollTimer);
        return;
      }
      job.status = info.status;
      job.report = info.report || null;
      job.metrics = info.metrics || null;
      job.chart = info.chart || null;
      job.logs = info.logs || null;
      job.error = info.error || null;
      // C8: 多维寻优才有 progress(`{done,total}`); 其余作业该字段不出现 → 保持 null。
      job.progress = info.progress || null;
      // 054 P4: 验证作业的结果在 `validate` 字段(不在 metrics —— 一次验证有 N 段)。
      job.validate = info.validate || null;
      renderJob(job);
      if (info.status !== "running" && st.pollTimer) clearInterval(st.pollTimer);
    };
    st.pollTimer = setInterval(() => {
      tick().catch(() => {});
    }, POLL_MS);
  }

  /// 三态渲染: running 转圈 / done 指标卡+权益曲线+可折叠文本报告 / error 原因+重试。
  /// 寻优作业另走对比表渲染 (`kind==="sweep"` 单参数 / `"optimize"` 多维)。
  function renderJob(job) {
    // 054 P4: 三类"另开一个作业槽"的结果各有自己的容器 —— **必须先分发再碰 `sg-bt-result`**。
    // 之前这里先清 `sg-bt-result` 再分发, 于是寻优/检验一跑完就把用户刚看的回测卡擦掉了。
    if (job.kind === "sweep") {
      renderSweepResult(job);
      return;
    }
    if (job.kind === "optimize") {
      renderOptimizeResult(job);
      return;
    }
    if (job.kind === "validate") {
      renderValidateResult(job);
      return;
    }
    const box = $("sg-bt-result");
    destroyBtChart();
    box.innerHTML = "";
    if (job.status === "running") {
      const row = h("div", "sg-job running");
      row.appendChild(h("span", "sg-spinner"));
      row.appendChild(h("span", null, t("sgBtRunning")));
      box.appendChild(row);
      return;
    }
    if (job.status === "done") {
      box.appendChild(h("div", "sg-job-head sg-ok", t("sgBtDone")));
      const m = job.metrics;
      if (m) {
        const label = job.strategyLabel || (st.detailCtx && st.detailCtx.id) || "";
        renderMetricCards(box, m);
        // 054 P3-4: 生效成本假设(含"滑点为 0"的点名) —— 紧挨指标, 不折叠。
        renderCostLine(box, job.cost);
        // C9: 与"同策略同窗口"的上一次结果并排(会话内); 首次跑时不出现。
        renderCompare(box, m, label);
        pushHistory(m, label);
        // 零成交不是"策略不好", 多为配置/条件问题 —— 明说, 不让用户对着平线猜。
        if ((m.total_trades ?? 0) === 0) {
          box.appendChild(h("div", "sg-hint", t("sgZeroTradeHint")));
        }
      }
      if (job.chart && job.chart.times && job.chart.times.length) {
        renderBtChart(box, job.chart);
      }
      // 039 FR-7: 逐笔平仓明细(无事件时如实提示, 不渲染空表)。
      if (job.chart) {
        renderClosedTrades(box, job.chart, m);
      }
      // 策略日志 (ctx:log): 让"0 成交/停机"有可解释原因 —— 缺 start_price 的 [FATAL] 就在其中。
      // 零成交或有 [FATAL] 时默认展开, 否则折叠(逐 tick 日志可能很长)。
      renderStrategyLogs(box, job.logs, m && (m.total_trades ?? 0) === 0);
      // 文本报告收进折叠块(仍与 CLI 同一字逐同源, 供细读)。
      const det = h("details", "sg-advanced");
      const sum = h("summary", null, t("sgReportFull"));
      det.appendChild(sum);
      const pre = h("pre", "sg-report");
      pre.textContent = job.report || "";
      // A2: 完整报告里的每一个指标名都可点出解释 —— 这份报告与 CLI 逐字同源, 术语表的
      // key 就是照它的指标名建的, 命中率最高。**要显式 allowPre**: 默认跳过 `<pre>`
      // 是为了不把 AI 气泡里的代码块当术语(那是标识符), 而这里装的是报告不是代码。
      R.glossary.decorate(pre, { allowPre: true });
      det.appendChild(pre);
      box.appendChild(det);
      // 回测闭环(P0-3): 一键把结论带给 AI 修改卡。
      if (m) {
        const row = h("div", "sg-row-right");
        const btn = h("button", "sg-btn sg-btn-mini", t("sgSendToAi"));
        btn.type = "button";
        btn.addEventListener("click", () => sendResultToAi(m));
        row.appendChild(btn);
        box.appendChild(row);
      }
      return;
    }
    // error
    box.appendChild(h("div", "sg-job-head sg-err", t("sgBtError")));
    box.appendChild(h("div", "sg-msg err", job.error || ""));
    const retry = h("button", "sg-btn sg-btn-mini", t("sgBtRetry"));
    retry.type = "button";
    retry.addEventListener("click", () => rerunJob(job).catch((e) => setMsg($("sg-bt-msg"), e.message, "err")));
    box.appendChild(retry);
  }

  /// 策略日志块 (2026-10-05): 折叠展示 `ctx:log` 输出。含 [FATAL]/停机 或零成交时默认展开,
  /// 让用户一眼看到"为什么没成交 / 为什么停机"(如内置香农缺 start_price 的 [FATAL])。
  function renderStrategyLogs(box, logs, forceOpen) {
    if (!logs || !logs.length) {
      if (forceOpen) {
        box.appendChild(h("div", "sg-msg", t("sgStrategyLogsEmpty")));
      }
      return;
    }
    const fatal = logs.some((l) => l.indexOf("[FATAL]") >= 0 || l.indexOf("停机") >= 0);
    const det = h("details", "sg-advanced sg-logs");
    if (fatal || forceOpen) det.open = true;
    det.appendChild(h("summary", null, t("sgStrategyLogs") + " (" + logs.length + ")"));
    det.appendChild(h("div", "sg-hint", t("sgStrategyLogsHint")));
    // 数据只经 textContent/createTextNode 注入 (项目前端纪律: 不 innerHTML 用户数据)。
    // [FATAL]/停机 行单独着色, 便于一眼定位停机原因。
    const pre = h("pre", "sg-report sg-logs-body");
    for (const line of logs) {
      if (line.indexOf("[FATAL]") >= 0 || line.indexOf("停机") >= 0) {
        const span = h("span", "sg-log-fatal");
        span.textContent = line + "\n";
        pre.appendChild(span);
      } else {
        pre.appendChild(document.createTextNode(line + "\n"));
      }
    }
    det.appendChild(pre);
    box.appendChild(det);
  }

  /// 指标卡(P0-2): 颜色遵循涨红跌绿(中国行情惯例); 中性值不着色。
  function renderMetricCards(box, m) {
    box.appendChild(h("div", "sg-mcards-title", t("sgMetrics")));
    const grid = h("div", "sg-mcards");
    const cards = [
      [t("sgEqVsBench"), fmtPct(m.equity_change_pct), signCls(m.equity_change_pct),
        m.benchmark_return_pct != null ? t("sgBenchShort") + " " + fmtPct(m.benchmark_return_pct) : ""],
      [t("sgNetPnl"), fmtNum(m.net_pnl), signCls(m.net_pnl), ""],
      [t("sgMaxDd"), fmtPct(-Math.abs(m.max_drawdown_pct || 0)),
        (m.max_drawdown_pct || 0) > 0 ? "down" : "flat", ""],
      [t("sgWinRate"), fmtPct(m.win_rate), "flat", ""],
      [t("sgTrades"), String(m.total_trades ?? 0), "flat",
        (m.rejected_count > 0 ? t("sgRejected") + " " + m.rejected_count : "")],
      [t("sgSharpe"), R.fmt.num(m.sharpe, 2), "flat", ""],
    ];
    for (const [label, value, cls, sub] of cards) {
      const card = h("div", "sg-mcard");
      // A2: 指标名挂术语(A2 起术语索引是全局单例) —— **命中术语表的才变可点**, 其余原样。
      // 于是"最大回撤 / 净盈亏 / 胜率"在指标卡上就能点出解释, 不必回对话里问。
      const lbl = h("span", "sg-mcard-label", label);
      R.glossary.decorate(lbl);
      card.appendChild(lbl);
      const v = h("span", "sg-mcard-value" + (cls ? " " + cls : ""), value);
      card.appendChild(v);
      if (sub) card.appendChild(h("span", "sg-mcard-sub", sub));
      grid.appendChild(card);
    }
    box.appendChild(grid);
  }

  /// 054 P3-4: 成本假设行 —— 就摆在指标卡**下面**(不是折叠里)。
  ///
  /// 为什么必须贴着指标: 滑点与限价穿透默认是 0(乐观), 网格/高频类策略的收益会被系统性高估。
  /// 用户看着一条漂亮的权益曲线时, 如果不知道这是零滑点下的数字, 后面所有判断都是错的 ——
  /// 这一行比任何免责声明都有效, 折叠起来等于没写。
  function renderCostLine(box, cost) {
    if (!cost) return;
    const n = (v, d) => (v == null ? "—" : R.fmt.num(Number(v), d == null ? 2 : d));
    const wrap = h("div", "sg-cost-box");
    wrap.appendChild(h("div", "sg-cost-title", t("sgCostTitle")));
    const line = h("div", "sg-cost-line");
    const parts = [
      [t("sgCostFee"), n(cost.fee_maker_bps, 1) + " / " + n(cost.fee_taker_bps, 1) + " bps"],
      [t("sgCostSlip"), n(cost.slippage_bps, 1) + " bps"],
      [t("sgCostPen"), n(cost.limit_fill_penetration_bps, 1) + " bps"],
      [t("sgCostCash"), n(cost.initial_cash, 0)],
      [t("sgCostLev"), n(cost.leverage, 1) + "x"],
    ];
    for (const [k, v] of parts) {
      const item = h("span", "sg-cost-item");
      item.appendChild(h("span", "sg-cost-key", k));
      item.appendChild(h("span", "sg-cost-val", v));
      line.appendChild(item);
    }
    wrap.appendChild(line);
    // 滑点为 0 必须**单独点名**: 混在一行里等于没说。
    if (cost.zero_slippage) {
      wrap.appendChild(h("div", "sg-cost-warn", t("sgCostZeroSlip")));
    }
    // 免责: 与 AI 评估提示词第 5 条同源(两处说法必须一致, 否则用户会挑一个好听的信)。
    wrap.appendChild(h("div", "sg-cost-note", t("sgBtDisclaimer")));
    box.appendChild(wrap);
  }

  /// 055 B3: 档位目标 vs 实测。
  ///
  /// 为什么必须摆出来: 用户选「保守」时, 平台替他把这句话翻译成了"回撤目标 ≤10%"并写进提示词,
  /// 但**说过了就该回头核对** —— 只把实测的 25% 显示成一个红数字, 等于让用户自己记住选了什么
  /// 再去比对。档位对用户的作用就断在这里。
  ///
  /// 措辞纪律: 判语是"未达到目标"而不是"不合规/失败" —— 档位写的是目标不是断言;
  /// 测不出来就说测不出来, 不摆一个 0 或 — 冒充实测值。
  function renderTierCheck(box, tc) {
    if (!tc || !tc.items || !tc.items.length) return;
    const wrap = h("div", "sg-tiercheck" + (tc.unmet > 0 ? " sg-tiercheck-unmet" : ""));
    wrap.appendChild(h("div", "sg-tiercheck-title", t("sgTierCheckTitle")));
    for (const it of tc.items) {
      const row = h("div", "sg-tiercheck-row");
      row.appendChild(h("span", "sg-tiercheck-name", R.lang === "en" ? it.label_en : it.label_zh));
      row.appendChild(
        h("span", "sg-tiercheck-target", t("sgTierCheckTarget") + " ≤ " + fmtNum2(it.target) + it.unit)
      );
      const actual =
        it.actual == null
          ? "—"
          : fmtNum2(it.actual) + it.unit;
      row.appendChild(h("span", "sg-tiercheck-actual", t("sgTierCheckActual") + " " + actual));
      const verdict =
        it.verdict === "unmet"
          ? t("sgTierCheckUnmet")
          : it.verdict === "met"
            ? t("sgTierCheckMet")
            : t("sgTierCheckUnknown");
      row.appendChild(h("span", "sg-tiercheck-verdict sg-tiercheck-" + it.verdict, verdict));
      wrap.appendChild(row);
    }
    // 未达标时给一条可行动的下一步; 达标时把"只核了哪些项"说清(免得用户以为全都核过)。
    wrap.appendChild(
      h("div", "sg-tiercheck-note", tc.unmet > 0 ? t("sgTierCheckUnmetNote") : t("sgTierCheckNote"))
    );
    box.appendChild(wrap);
  }

  function fmtNum2(v) {
    return R.fmt.num(Number(v), 2);
  }

  /// 054 P1/T6/T7: 把一次「验证回测」的结果渲染进任意容器(生成弹窗用)。
  ///
  /// **失败时绝不显示任何指标卡** —— 宁可空着, 也不能让用户以为已验证(与实时行情三态 chip
  /// 同一纪律: 不静默降级)。
  function renderVerifyResult(box, verify) {
    box.innerHTML = "";
    if (!verify) {
      box.appendChild(h("div", "sg-hint", t("sgVerifySkipHint")));
      return;
    }
    if (!verify.ok) {
      const head = h("div", "sg-job-head sg-err", t("sgGenUnverified"));
      box.appendChild(head);
      box.appendChild(h("div", "sg-hint", t("sgGenUnverifiedHint")));
      if (verify.reason) {
        const pre = h("div", "sg-verify-reason", verify.reason); // 服务端文本走 textContent
        box.appendChild(pre);
      }
      return;
    }
    box.appendChild(h("div", "sg-job-head sg-ok", t("sgGenVerified")));
    if (verify.metrics) {
      renderMetricCards(box, verify.metrics);
    }
    // 055 B3: 紧贴指标卡 —— 先看到数字, 立刻看到"达到你选的那档了吗"。
    renderTierCheck(box, verify.tier_check);
    renderCostLine(box, verify.cost);
    if (verify.chart && verify.chart.times && verify.chart.times.length) {
      renderBtChart(box, verify.chart);
    }
    if (verify.metrics && (verify.metrics.total_trades ?? 0) === 0) {
      box.appendChild(h("div", "sg-hint", t("sgZeroTradeHint")));
    }
  }

  /// 权益曲线 vs 标的价格(P0-2): 两者同 quote 计价共轴; 买卖点标记在价格线上。
  /// 039 增补: 基准(买入持有)对照线 + 逐点回撤副图 + 图例。
  function renderBtChart(box, chart) {
    const wrap = h("div", "sg-bt-chart-wrap");
    // 039: 图例 —— 颜色与线一致(CSS 变量), 免去"这条虚线是什么"的猜测。
    const legend = h("div", "sg-bt-legend");
    for (const [cls, key] of [
      ["lg-price", "sgLegPrice"],
      ["lg-equity", "sgLegEquity"],
      ["lg-bench", "sgLegBench"],
      ["lg-dd", "sgLegDd"],
    ]) {
      const item = h("span", "lg " + cls, t(key));
      // A2: 图例名也挂术语 —— "标的价格"是词条, "回撤"不是(术语表里叫"最大回撤"), 命中才变可点。
      R.glossary.decorate(item);
      legend.appendChild(item);
    }
    wrap.appendChild(legend);
    const div = h("div", "sg-bt-chart");
    wrap.appendChild(div);
    box.appendChild(wrap);
    const border = themeColor("--border", "#f0f0f0");
    const LWC = window.LightweightCharts;
    st.btChart = LWC.createChart(div, {
      autoSize: true,
      layout: { background: { color: "transparent" }, textColor: themeColor("--muted", "#5b6672") },
      grid: { vertLines: { color: border }, horzLines: { color: border } },
      timeScale: { timeVisible: true, secondsVisible: false, borderColor: border },
    });
    const sec = (ms) => Math.floor(ms / 1000);
    // 价格线(muted 细线)。
    const price = st.btChart.addLineSeries({
      color: themeColor("--muted", "#5b6672"),
      lineWidth: 1,
      priceLineVisible: false,
      lastValueVisible: false,
    });
    price.setData(
      chart.times
        .map((t, i) => ({ time: sec(t), value: chart.price[i] }))
        .filter((d) => Number.isFinite(d.value) && Number.isFinite(d.time))
        .sort((a, b) => a.time - b.time)
    );
    // 权益线(主色粗线 —— 主色不是涨跌色, 故仍引 `--accent`)。
    const eq = st.btChart.addLineSeries({
      color: themeColor("--accent", "#1caf83"),
      lineWidth: 2,
      priceLineVisible: false,
    });
    eq.setData(
      chart.times
        .map((t, i) => ({ time: sec(t), value: chart.equity[i] }))
        .filter((d) => Number.isFinite(d.value) && Number.isFinite(d.time))
        .sort((a, b) => a.time - b.time)
    );
    // ---- 039 FR-5: 基准线(买入持有, 自首次成交起同本金) ----
    // 与指标卡 benchmark_return_pct 同一口径, 由后端算好下发; 建仓前的点是 null →
    // 只喂 time(whitespace 点), 曲线自然从建仓那根开始, **不**画成 0。
    if (chart.benchmark && chart.benchmark.length) {
      const bench = st.btChart.addLineSeries({
        color: themeColor("--warn", "#96660a"),
        lineWidth: 1,
        lineStyle: 2, // 虚线: 一眼区别于策略权益与标的价格
        priceLineVisible: false,
        lastValueVisible: false,
      });
      bench.setData(
        chart.times
          .map((t, i) => {
            const v = chart.benchmark[i];
            return Number.isFinite(v) ? { time: sec(t), value: v } : { time: sec(t) };
          })
          .filter((d) => Number.isFinite(d.time))
          .sort((a, b) => a.time - b.time)
      );
    }
    // ---- 039 FR-6: 逐点回撤(水下曲线) ----
    // 值 ≤ 0, 0 在副图顶部; 由后端用**全分辨率**曲线算完再抽稀(口径 = 指标卡的 max_drawdown)。
    if (chart.drawdown && chart.drawdown.length) {
      const dd = st.btChart.addAreaSeries({
        priceScaleId: "dd",
        lineColor: themeColor("--error", "#f85149"),
        topColor: "transparent",
        bottomColor: themeColor("--error", "#f85149"),
        lineWidth: 1,
        priceLineVisible: false,
        lastValueVisible: false,
        priceFormat: { type: "custom", formatter: (v) => (v * 100).toFixed(1) + "%" },
      });
      st.btChart.priceScale("dd").applyOptions({ scaleMargins: { top: 0.78, bottom: 0 } });
      dd.setData(
        chart.times
          .map((t, i) => ({ time: sec(t), value: chart.drawdown[i] }))
          .filter((d) => Number.isFinite(d.value) && Number.isFinite(d.time))
          .sort((a, b) => a.time - b.time)
      );
    }
    // 买卖点(涨红跌绿: 买=红 上箭头, 卖=绿 下箭头)。
    // 062: 涨跌色拆到 `--up`/`--down` —— 不再借 `--error`(危险语义)与 `--accent`(主色)。
    const buyColor = themeColor("--up", "#f5222d");
    const sellColor = themeColor("--down", "#0d9b6c");
    const markers = (chart.fills || [])
      .map((f) => ({
        time: sec(f.t),
        position: f.side === "buy" ? "belowBar" : "aboveBar",
        color: f.side === "buy" ? buyColor : sellColor,
        shape: f.side === "buy" ? "arrowUp" : "arrowDown",
        text: f.side === "buy" ? "B" : "S",
      }))
      .sort((a, b) => a.time - b.time);
    if (markers.length) price.setMarkers(markers);
    st.btChart.timeScale().fitContent();
  }

  /// 039 FR-7/FR-8: 平仓盈亏明细表 —— 逐笔列出平仓事件时刻与已实现盈亏。
  ///
  /// 明细与指标卡**同源**(引擎一次 record_pnl 既进聚合也进明细), 故可给出可核对的一致性副标:
  /// 未被截断时 求和 == 已实现盈亏; 被截断时明说"仅显示最近 N 条(共 M 条)"并只报**显示部分**的和,
  /// 不许拿部分和冒充总数(诚实性硬约束)。
  function renderClosedTrades(box, chart, m) {
    const rows = chart.closed || [];
    if (!rows.length) {
      // 0 行不渲染空表 —— 直接说清"没有平仓事件"。
      box.appendChild(h("div", "sg-hint", t("sgClosedEmpty")));
      return;
    }
    const total = Number.isFinite(chart.closed_total) ? chart.closed_total : rows.length;
    const truncated = total > rows.length;
    box.appendChild(h("div", "sg-mcards-title", t("sgClosedTitle") + " (" + rows.length + ")"));
    const wrap = h("div", "sg-closed-wrap");
    const tbl = h("table", "sg-hist-table");
    const thead = h("thead");
    const hr = h("tr");
    hr.appendChild(h("th", null, t("sgClosedThTime")));
    // A2: 表头"已实现盈亏"是术语表词条 —— 点一下就知道它和"净盈亏"差在哪。
    const thPnl = h("th", null, t("sgClosedThPnl"));
    R.glossary.decorate(thPnl);
    hr.appendChild(thPnl);
    thead.appendChild(hr);
    tbl.appendChild(thead);
    const tbody = h("tbody");
    // 最新一笔在最上面(与"刚跑完想看最近发生了什么"一致); 数据仍是时间升序下发, 这里倒序渲染。
    for (const row of rows.slice().reverse()) {
      const tr = h("tr");
      tr.appendChild(h("td", null, fmtMs(row.t)));
      const cls = row.pnl > 0 ? "up" : row.pnl < 0 ? "down" : "";
      tr.appendChild(h("td", cls, fmtNum(row.pnl)));
      tbody.appendChild(tr);
    }
    tbl.appendChild(tbody);
    wrap.appendChild(tbl);
    box.appendChild(wrap);
    const shownSum = rows.reduce((acc, r) => acc + (Number.isFinite(r.pnl) ? r.pnl : 0), 0);
    let note = "";
    if (truncated) {
      note =
        t("sgClosedTrunc").replace("{n}", String(rows.length)).replace("{m}", String(total)) +
        " · " +
        t("sgClosedShownSum") +
        fmtNum(shownSum);
    } else {
      note = t("sgClosedSum") + fmtNum(shownSum);
      // 未截断时与指标卡交叉核对; 对不上如实提示(不静默)。
      if (m && Number.isFinite(m.realized_pnl) && Math.abs(m.realized_pnl - shownSum) > 0.01) {
        note += " · " + t("sgClosedMismatch") + fmtNum(m.realized_pnl) + ")";
      }
    }
    const sub = h("div", "sg-closed-sub", note);
    R.glossary.decorate(sub); // A2: 这行说明里的指标名(如"已实现盈亏")同样可点
    box.appendChild(sub);
  }

  function destroyValChart() {
    if (st.valChart) {
      st.valChart.remove();
      st.valChart = null;
    }
  }

  function destroyBtChart() {
    if (st.btChart) {
      st.btChart.remove();
      st.btChart = null;
    }
  }

  /// 图表配色跟随主题(同 markets.js 的取法)。
  function themeColor(name, fallback) {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  }

  // C11: 以下三个格式化函数的**实现**统一到 `R.fmt`, 调用点与签名不变(委托形态) ——
  // 出口只有一个, 但各视图仍用自己读得懂的名字。
  function fmtPct(v) {
    return R.fmt.pct(v);
  }

  function fmtNum(v) {
    return R.fmt.signed(v);
  }

  /// 毫秒时间戳 → 展示串 (平仓明细用)。会话列表的秒级口径走 `R.formatTime`(= `R.fmt.timeSec`),
  /// 两者口径不同(缺值一个是 `—` 一个是空), 在 `R.fmt` 里是**两个方法**, 不是一份实现。
  function fmtMs(ms) {
    return R.fmt.time(ms);
  }

  function signCls(v) {
    if (v == null || !Number.isFinite(v) || v === 0) return "flat";
    return v > 0 ? "up" : "down";
  }

  /// 回测结论带给 AI(P0-3 闭环): 把关键指标拼成一句修改请求填进 AI 指令框。
  function sendResultToAi(m) {
    const parts = [
      "刚完成的回测结果: 权益变化 " + fmtPct(m.equity_change_pct),
      "买入持有基准 " + (m.benchmark_return_pct != null ? fmtPct(m.benchmark_return_pct) : "无"),
      "最大回撤 " + fmtPct(Math.abs(m.max_drawdown_pct || 0)),
      "夏普 " + (m.sharpe != null ? R.fmt.num(m.sharpe, 2) : "无"),
      "胜率 " + fmtPct(m.win_rate),
      "成交 " + (m.total_trades ?? 0) + " 笔",
    ];
    const ta = $("sg-ai-instr");
    ta.value = parts.join(", ") + "。请基于这份结果改进策略(例如先提高收益/降低回撤/减少无效交易)。";
    ta.scrollIntoView({ block: "center" });
    ta.focus();
  }

  // ---------- C9: 与上次结果对比 ----------

  /// 找"同策略 + 同交易对 + 同周期 + 同窗口"的上一次结果(仅本次会话的内存)。
  /// 窗口不同**不比** —— 拿 7 天与 30 天的收益比大小是误导, 宁可不说。
  function findPrev(m, label) {
    if (!label) return null;
    for (const r of st.jobHistory) {
      if (
        r.strategy === label &&
        r.pair === m.pair &&
        r.interval === m.interval &&
        r.days === m.days
      ) {
        return r;
      }
    }
    return null;
  }

  /// C9: 关键指标并排(上次 / 本次 / 变化), 变化按**涨红跌绿**着色(与指标卡同一套 `signCls`)。
  /// 数据源是会话内历史(`st.jobHistory`), **服务端不存回测归档** —— 刷新页面即清空, 这一条
  /// 也在文案里说清("本次会话"口径), 不假装是长期对比。
  function renderCompare(box, m, label) {
    const prev = findPrev(m, label);
    if (!prev) return;
    const fmtVal = (v, kind) => {
      if (v == null || !Number.isFinite(Number(v))) return "—";
      if (kind === "int") return String(v);
      if (kind === "num") return R.fmt.num(v, 2);
      return R.fmt.pct(v);
    };
    const fmtDelta = (d, kind) => {
      if (d == null) return "—";
      if (kind === "int") return (d > 0 ? "+" : "") + String(d);
      if (kind === "num") return R.fmt.signed(d);
      return R.fmt.pct(d);
    };
    const rows = [
      ["sgEqVsBench", prev.equity_change_pct, m.equity_change_pct, "pct"],
      // 回撤取负(指标卡同款: 更深的回撤数值更小), 这样"变化"的正负语义与其它指标一致。
      ["sgMaxDd", -Math.abs(prev.max_drawdown_pct || 0), -Math.abs(m.max_drawdown_pct || 0), "pct"],
      ["sgSharpe", prev.sharpe, m.sharpe, "num"],
      ["sgWinRate", prev.win_rate, m.win_rate, "pct"],
      ["sgTrades", prev.total_trades, m.total_trades, "int"],
    ];
    const card = h("section", "sg-card");
    card.appendChild(h("div", "sg-card-head", t("sgCmpTitle")));
    const cbody = h("div", "sg-card-body");
    cbody.appendChild(h("div", "sg-hint", t("sgCmpHint")));
    const table = h("table", "sg-hist-table sg-cmp-table");
    const thead = h("thead");
    const hr = h("tr");
    for (const k of ["sgCmpMetric", "sgCmpPrev", "sgCmpNow", "sgCmpDelta"]) {
      hr.appendChild(h("th", null, t(k)));
    }
    thead.appendChild(hr);
    table.appendChild(thead);
    const tbody = h("tbody");
    for (const [key, pv, nv, kind] of rows) {
      const tr = h("tr");
      tr.appendChild(h("td", null, t(key)));
      tr.appendChild(h("td", "num", fmtVal(pv, kind)));
      tr.appendChild(h("td", "num", fmtVal(nv, kind)));
      const both =
        pv != null && nv != null && Number.isFinite(Number(pv)) && Number.isFinite(Number(nv));
      const delta = both ? Number(nv) - Number(pv) : null;
      tr.appendChild(h("td", "num " + signCls(delta), fmtDelta(delta, kind)));
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    cbody.appendChild(table);
    card.appendChild(cbody);
    box.appendChild(card);
  }

  // ---------- 回测历史对比 (P1-6) ----------

  /// 只进内存(会话级), 不写浏览器持久存储(SC-011); 最多 20 条。
  function pushHistory(m, strategyLabel) {
    st.jobHistory.unshift({
      ts: Date.now(),
      strategy: strategyLabel,
      pair: m.pair,
      interval: m.interval,
      days: m.days,
      equity_change_pct: m.equity_change_pct,
      benchmark_return_pct: m.benchmark_return_pct,
      max_drawdown_pct: m.max_drawdown_pct,
      sharpe: m.sharpe,
      win_rate: m.win_rate,
      total_trades: m.total_trades,
    });
    if (st.jobHistory.length > 20) st.jobHistory.pop();
    renderHistory();
  }

  function renderHistory() {
    const box = $("sg-bt-history");
    if (!box) return;
    box.innerHTML = "";
    box.appendChild(h("div", "sg-hist-title", t("sgHistory")));
    if (!st.jobHistory.length) {
      box.appendChild(h("div", "sg-empty", t("sgHistoryEmpty")));
      return;
    }
    const table = h("table", "sg-hist-table");
    const thead = h("thead");
    const hr = h("tr");
    for (const label of [
      t("sgHistoryTime"), t("sgName"), t("sgHistoryWindow"), t("sgEqVsBench"),
      t("sgMaxDd"), t("sgSharpe"), t("sgWinRate"), t("sgTrades"),
    ]) {
      hr.appendChild(h("th", null, label));
    }
    thead.appendChild(hr);
    table.appendChild(thead);
    const tbody = h("tbody");
    for (const r of st.jobHistory) {
      const tr = h("tr");
      const time = new Date(r.ts);
      const hh = String(time.getHours()).padStart(2, "0");
      const mm = String(time.getMinutes()).padStart(2, "0");
      const cells = [
        hh + ":" + mm,
        r.strategy,
        r.pair + " " + r.interval + " " + r.days + "d",
        fmtPct(r.equity_change_pct),
        fmtPct(Math.abs(r.max_drawdown_pct || 0)),
        R.fmt.num(r.sharpe, 2),
        fmtPct(r.win_rate),
        String(r.total_trades ?? 0),
      ];
      cells.forEach((c, i) => {
        const td = h("td", null, c);
        if (i === 3) td.classList.add(signCls(r.equity_change_pct));
        tr.appendChild(td);
      });
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    box.appendChild(table);
  }

  // ---------- 参数寻优 (P2-7) ----------

  /// 发起单参数网格扫描: 每档 = 一次完整回测(与单次回测同内核同口径), 结果进对比表。
  async function onSweep() {
    if (!st.detailCtx) return;
    const param = $("sg-sw-param").value.trim();
    const raw = $("sg-sw-values").value.trim();
    if (!param || !raw) {
      setMsg($("sg-sw-msg"), t("sgSweepEmpty"), "err");
      return;
    }
    const values = raw
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
      .map((s) => (Number.isFinite(Number(s)) ? Number(s) : s));
    const body = {
      strategy: st.detailCtx.id,
      param: param,
      values: values,
      pair: $("sg-bt-pair").value.trim() || $("sg-pair").value.trim(),
      interval: $("sg-bt-interval").value,
      days: Number($("sg-bt-days").value.trim()) || 90,
    };
    const fee = Number($("sg-bt-fee").value.trim());
    if (Number.isFinite(fee) && $("sg-bt-fee").value.trim() !== "") body.fee = fee;
    const cash = Number($("sg-bt-cash").value.trim());
    if (Number.isFinite(cash) && $("sg-bt-cash").value.trim() !== "") body.cash = cash;
    const leverage = Number($("sg-bt-leverage").value.trim());
    if (Number.isFinite(leverage) && $("sg-bt-leverage").value.trim() !== "") body.leverage = leverage;

    const g = st.gen;
    const btn = $("sg-sw-go");
    btn.disabled = true;
    setMsg($("sg-sw-msg"), "", "");
    try {
      const reply = await R.api("/api/backtest/sweep", { method: "POST", body: body });
      if (g !== st.gen) return;
      const key = st.detailCtx.id + "#sweep";
      st.jobs.set(key, { jobId: reply.job_id, status: "running", kind: "sweep", body: body });
      startPolling(key, reply.job_id, g);
    } catch (err) {
      if (g !== st.gen) return;
      setMsg($("sg-sw-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === st.gen) btn.disabled = false;
    }
  }

  /// 寻优结果对比表: 权益变化最高档高亮; 单档失败原因如实入表。
  function renderSweepResult(job) {
    const box = $("sg-sw-result");
    box.innerHTML = "";
    if (job.status === "running") {
      const row = h("div", "sg-job running");
      row.appendChild(h("span", "sg-spinner"));
      row.appendChild(h("span", null, t("sgSweepRunning")));
      box.appendChild(row);
      return;
    }
    if (job.status === "error") {
      box.appendChild(h("div", "sg-msg err", job.error || ""));
      return;
    }
    let rows = [];
    try {
      rows = JSON.parse(job.report || "[]");
    } catch (_) {
      rows = [];
    }
    if (!rows.length) return;
    let best = null;
    for (const r of rows) {
      if (r.metrics && Number.isFinite(r.metrics.equity_change_pct)) {
        if (best === null || r.metrics.equity_change_pct > best.metrics.equity_change_pct) best = r;
      }
    }
    const table = h("table", "sg-hist-table");
    const thead = h("thead");
    const hr = h("tr");
    for (const label of [
      t("sgSweepValue"), t("sgEqVsBench"), t("sgNetPnl"), t("sgMaxDd"),
      t("sgSharpe"), t("sgWinRate"), t("sgTrades"),
    ]) {
      hr.appendChild(h("th", null, label));
    }
    thead.appendChild(hr);
    table.appendChild(thead);
    const tbody = h("tbody");
    for (const r of rows) {
      const tr = h("tr");
      if (r === best) tr.classList.add("sg-best");
      if (r.error) {
        const td = h("td", null, String(r.value));
        td.colSpan = 7;
        const err = h("span", "sg-msg err", r.error);
        td.appendChild(err);
        tr.appendChild(td);
      } else {
        const m = r.metrics || {};
        const cells = [
          String(r.value),
          fmtPct(m.equity_change_pct),
          fmtNum(m.net_pnl),
          fmtPct(Math.abs(m.max_drawdown_pct || 0)),
          R.fmt.num(m.sharpe, 2),
          fmtPct(m.win_rate),
          String(m.total_trades ?? 0),
        ];
        cells.forEach((c, i) => {
          const td = h("td", null, c);
          if (i === 1) td.classList.add(signCls(m.equity_change_pct));
          tr.appendChild(td);
        });
      }
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    box.appendChild(table);
    setMsg($("sg-sw-msg"), t("sgBtDone"), "ok");
  }

  // ---------- C8: 多维参数寻优 ----------

  /// 结果表里可排序的指标列。`dir` = 该指标"更优"的方向(回撤越小越好, 所以是 +1)。
  /// 复用指标卡那套文案键, 不另起一套名字 —— 同一个数字在页面上只有一个说法。
  const OPT_COLS = [
    { key: "equity_change_pct", label: "sgEqVsBench", kind: "pct", dir: -1 },
    { key: "net_pnl", label: "sgNetPnl", kind: "num", dir: -1 },
    { key: "max_drawdown_pct", label: "sgMaxDd", kind: "pct", dir: 1 },
    { key: "sharpe", label: "sgSharpe", kind: "num", dir: -1 },
    { key: "total_trades", label: "sgTrades", kind: "int", dir: -1 },
  ];

  /// 当前排序(模块级: 只影响这一屏怎么摆, 不进 `st`)。默认按权益变化从高到低。
  let optSort = { key: "equity_change_pct" };
  /// 上一次解析出的结果行 —— 点表头排序时不必再问服务端(结果是一次性的, 过期就 404 了)。
  let optRows = null;

  /// 取某行的某指标(回撤取绝对值 —— 服务端给的是负数, 直接摆出来会被当成"亏损")。
  /// 算不出(失败行 / 缺值) → null。
  function optVal(row, key) {
    const m = row.metrics;
    if (!m) return null;
    let v = m[key];
    if (key === "max_drawdown_pct") v = Math.abs(v || 0);
    return Number.isFinite(v) ? v : null;
  }

  /// 按当前排序规则摆行: 失败行**恒定沉底**(它们没有指标, 混在中间会让人误以为"这一档很差")。
  function sortOptRows(rows) {
    const col = OPT_COLS.find((c) => c.key === optSort.key) || OPT_COLS[0];
    const sign = col.dir;
    const copy = rows.slice();
    copy.sort((a, b) => {
      const va = optVal(a, col.key);
      const vb = optVal(b, col.key);
      if (va === null && vb === null) return 0;
      if (va === null) return 1;
      if (vb === null) return -1;
      return sign * (vb - va);
    });
    return copy;
  }

  /// 画结果表: 每个维度一列(参数组合) + 指标列(表头可点排序) + 最优行高亮。
  function paintOptTable() {
    const box = $("sg-opt-result");
    if (!box || !optRows) return;
    box.textContent = "";
    if (!optRows.length) return;
    const dims = (optRows[0].combo || []).map((c) => c.key);
    const sorted = sortOptRows(optRows);
    const col = OPT_COLS.find((c) => c.key === optSort.key) || OPT_COLS[0];
    const best = sorted.find((r) => r.metrics && optVal(r, col.key) !== null) || null;

    const cap = h(
      "div",
      "sg-opt-cap",
      t("sgOptBest") + t(col.label) + t("sgOptBestSuffix")
    );
    box.appendChild(cap);

    const table = h("table", "sg-hist-table sg-opt-table");
    const thead = h("thead");
    const hr = h("tr");
    for (const key of dims) hr.appendChild(h("th", null, key));
    for (const c of OPT_COLS) {
      const th = h("th", "sg-opt-sort", t(c.label) + (c.key === optSort.key ? " ▾" : ""));
      th.title = t("sgOptSortHint");
      th.addEventListener("click", () => {
        optSort = { key: c.key };
        paintOptTable();
      });
      hr.appendChild(th);
    }
    thead.appendChild(hr);
    table.appendChild(thead);

    const tbody = h("tbody");
    let shown = 0; // 失败行数(如实报数, 不静默丢)
    for (const r of sorted) {
      const tr = h("tr");
      if (r === best) tr.classList.add("sg-best");
      for (const cell of r.combo || []) {
        tr.appendChild(h("td", "num", String(cell.value)));
      }
      if (r.error) {
        shown += 1;
        const td = h("td", "sg-msg err", r.error);
        td.colSpan = OPT_COLS.length;
        tr.appendChild(td);
      } else {
        for (const c of OPT_COLS) {
          const v = optVal(r, c.key);
          const text =
            v === null
              ? "—"
              : c.kind === "pct"
                ? fmtPct(v)
                : c.kind === "int"
                  ? String(v)
                  : fmtNum(v);
          const td = h("td", "num", text);
          if (c.kind === "pct" || c.kind === "num") td.classList.add(signCls(v));
          tr.appendChild(td);
        }
      }
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    box.appendChild(table);
    if (shown > 0) {
      box.appendChild(h("div", "sg-hint", t("sgOptErrRow") + ": " + shown));
    }
  }

  /// 发起多维寻优: 维度行由表单子文件读回, 组合数由后端兜底(超 100 会 400, 原样显示原因)。
  async function onOptimize() {
    if (!st.detailCtx) return;
    const params = S.readOptDims();
    if (!params || !params.length) {
      setMsg($("sg-opt-msg"), t("sgOptEmpty"), "err");
      return;
    }
    const body = {
      strategy: st.detailCtx.id,
      params: params,
      pair: $("sg-bt-pair").value.trim() || $("sg-pair").value.trim(),
      interval: $("sg-bt-interval").value,
      days: Number($("sg-bt-days").value.trim()) || 90,
    };
    const fee = Number($("sg-bt-fee").value.trim());
    if (Number.isFinite(fee) && $("sg-bt-fee").value.trim() !== "") body.fee = fee;
    const cash = Number($("sg-bt-cash").value.trim());
    if (Number.isFinite(cash) && $("sg-bt-cash").value.trim() !== "") body.cash = cash;
    const leverage = Number($("sg-bt-leverage").value.trim());
    if (Number.isFinite(leverage) && $("sg-bt-leverage").value.trim() !== "") {
      body.leverage = leverage;
    }

    const g = st.gen;
    const btn = $("sg-opt-go");
    btn.disabled = true;
    setMsg($("sg-opt-msg"), "", "");
    try {
      const reply = await R.api("/api/backtest/optimize", { method: "POST", body: body });
      if (g !== st.gen) return;
      const key = st.detailCtx.id + "#opt";
      optRows = null;
      st.jobs.set(key, { jobId: reply.job_id, status: "running", kind: "optimize", body: body });
      startPolling(key, reply.job_id, g);
    } catch (err) {
      if (g !== st.gen) return;
      setMsg($("sg-opt-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === st.gen) btn.disabled = false;
    }
  }

  /// 多维寻优三态: 跑 → 进度条(done/total) / 完 → 可排序对比表 + 最优行 / 错 → 原因。
  function renderOptimizeResult(job) {
    const box = $("sg-opt-result");
    if (!box) return;
    box.textContent = "";
    if (job.status === "running") {
      const row = h("div", "sg-job running");
      row.appendChild(h("span", "sg-spinner"));
      row.appendChild(h("span", null, t("sgOptRunning")));
      const p = job.progress;
      if (p && p.total) {
        row.appendChild(
          h("span", "sg-opt-progtext", t("sgOptProgress") + " " + p.done + "/" + p.total)
        );
        const bar = h("div", "sg-opt-prog");
        const fill = h("div", "sg-opt-prog-fill");
        fill.style.width = ((100 * p.done) / p.total).toFixed(1) + "%";
        bar.appendChild(fill);
        row.appendChild(bar);
      }
      box.appendChild(row);
      return;
    }
    if (job.status === "error") {
      box.appendChild(h("div", "sg-msg err", job.error || ""));
      return;
    }
    try {
      optRows = JSON.parse(job.report || "[]");
    } catch (_) {
      optRows = [];
    }
    if (!optRows.length) return;
    paintOptTable();
    setMsg($("sg-opt-msg"), t("sgBtDone"), "ok");
  }

  // ---------- 054 P4: 样本外检验 / 分段验证 / 滚动验证 ----------
  //
  // 为什么要有这一块(审计 #3 / #5): 用户跑完一次回测看到的数字, 全部来自**同一段**历史;
  // 参数寻优又是在这同一段上挑出来的最好一组 —— 于是"回测很漂亮"这件事几乎不携带关于未来
  // 的信息。这里把它做成"点一下就能看", 而不是要求用户先理解方法论。
  //
  // 三条纪律:
  //   1. 只按时间顺序切(随机/交错切会把未来信息漏进训练段, 自己制造前视偏差)—— 切法在后端;
  //   2. 术语分开: 「分段验证」(参数不变) 与「滚动验证(含重新调参)」是两件事, 不统称;
  //   3. 退化只描述不定性: 界面照抄服务端的 note, 不自己拼"过拟合"这类定性词。

  /// 发起验证作业(202 + job_id, 与回测共用队列与 409 busy)。
  async function onValidate() {
    if (!st.detailCtx) return;
    const mode = $("sg-val-mode").value;
    const form = mode === "walk" ? $("sg-val-form").value : "A";
    const body = {
      strategy: st.detailCtx.id,
      pair: $("sg-bt-pair").value.trim() || $("sg-pair").value.trim(),
      interval: $("sg-bt-interval").value,
      days: Number($("sg-bt-days").value.trim()) || 90,
      mode: mode,
      form: form,
    };
    // 形态 B 的候选网格**沿用多维寻优那几行**: 不让用户在第二处再填一遍(两处迟早不一致),
    // 也不让后端去猜 —— 猜出来的"最优参数"是平台的产物, 用户根本不知道它被调过什么。
    if (form === "B") {
      const dims = S.readOptDims();
      if (!dims || !dims.length) {
        setMsg($("sg-val-msg"), t("sgValDimsRequired"), "err");
        return;
      }
      body.dims = dims;
    }
    const g = st.gen;
    const btn = $("sg-val-go");
    btn.disabled = true;
    setMsg($("sg-val-msg"), "", "");
    try {
      const reply = await R.api("/api/backtest/validate", { method: "POST", body: body });
      if (g !== st.gen) return;
      const key = st.detailCtx.id + "#val";
      const job = {
        jobId: reply.job_id,
        status: "running",
        kind: "validate",
        body: body,
        strategyLabel: st.detailCtx.id,
      };
      st.jobs.set(key, job);
      renderJob(job);
      startPolling(key, reply.job_id, g);
    } catch (err) {
      if (g !== st.gen) return;
      setMsg($("sg-val-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === st.gen) btn.disabled = false;
    }
  }

  /// 一段的指标行(与普通回测的指标卡同源口径, 但只挑最能说明"这段怎么样"的四项)。
  function segMetricRow(m) {
    const wrap = h("div", "sg-val-mrow");
    if (!m) return wrap;
    const items = [
      [t("sgNetPnl"), fmtNum(m.net_pnl), signCls(m.net_pnl)],
      [t("sgMaxDd"), fmtPct(-Math.abs(m.max_drawdown_pct || 0)), (m.max_drawdown_pct || 0) > 0 ? "down" : "flat"],
      [t("sgTrades"), String(m.total_trades ?? 0), "flat"],
      [t("sgWinRate"), fmtPct(m.win_rate), "flat"],
    ];
    for (const [k, vv, cls] of items) {
      const item = h("span", "sg-val-mitem");
      item.appendChild(h("span", "sg-val-mkey", k));
      item.appendChild(h("span", "sg-val-mval" + (cls ? " " + cls : ""), vv));
      wrap.appendChild(item);
    }
    return wrap;
  }

  /// 离散度提示(T32): 只**描述**段与段的差异, 不断言原因 —— 差异大 ≠ 过拟合,
  /// 可能只是行情换了风格。定性词一律不出现在这里。
  function renderSpread(box, segs) {
    const vals = segs
      .map((s) => (s.metrics ? Number(s.metrics.net_pnl) : NaN))
      .filter((v) => Number.isFinite(v));
    if (vals.length < 2) {
      box.appendChild(h("div", "sg-hint", t("sgValNoSpread")));
      return;
    }
    const min = Math.min.apply(null, vals);
    const max = Math.max.apply(null, vals);
    const line =
      t("sgValSpread") +
      fmtNum(min) +
      t("sgValSpreadMid") +
      fmtNum(max) +
      t("sgValSpreadTail") +
      fmtNum(max - min) +
      t("sgValSpreadEnd");
    box.appendChild(h("div", "sg-hint", line));
  }

  /// 拼接各段曲线 → 一条线。段间有空档(形态 B 跳过第一段训练区)时插**空白点**断开,
  /// 否则直连会画出一段"策略其实没在里面"的斜线 —— 那等于编造。
  function stitchCurve(segs) {
    const pts = [];
    for (const s of segs) {
      const c = s.curve;
      if (!c || !c.t || !c.eq) continue;
      const startMs = Date.parse(s.start + "T00:00:00Z");
      if (pts.length && Number.isFinite(startMs)) {
        const last = pts[pts.length - 1];
        // 上一段的终点早于这一段的起点 → 中间是空档。
        if (last.time < Math.floor(startMs / 1000) - 1) {
          pts.push({ time: Math.floor(startMs / 1000) - 1 });
        }
      }
      for (let i = 0; i < c.t.length; i += 1) {
        const v = Number(c.eq[i]);
        if (Number.isFinite(v)) pts.push({ time: Math.floor(c.t[i] / 1000), value: v });
      }
    }
    return pts.sort((a, b) => a.time - b.time);
  }

  /// 两条线并排(T32): 本次检验拼接线 + 同一组参数跑全窗口的线(取自会话内最近一次
  /// **同窗口同周期**的普通回测)。窗口/周期对不上就不画 —— 那两条线没有可比性,
  /// 画上去只会让人做出错误的比较。
  function renderValChart(box, v, strategyId) {
    const ours = stitchCurve(v.segments || []);
    if (!ours.length) return;
    const full0 = st.jobs.get(strategyId);
    const fm = full0 && full0.metrics;
    const ok =
      full0 &&
      full0.status === "done" &&
      full0.chart &&
      fm &&
      fm.interval === v.interval &&
      fm.days === v.days;
    const wrap = h("div", "sg-bt-chart-wrap");
    wrap.appendChild(h("div", "sg-mcards-title", t("sgValCurveTitle")));
    const legend = h("div", "sg-bt-legend");
    legend.appendChild(h("span", "lg lg-equity", t("sgValCurveOurs")));
    if (ok) legend.appendChild(h("span", "lg lg-bench", t("sgValCurveFull")));
    wrap.appendChild(legend);
    const div = h("div", "sg-bt-chart");
    wrap.appendChild(div);
    box.appendChild(wrap);
    const border = themeColor("--border", "#f0f0f0");
    const LWC = window.LightweightCharts;
    destroyValChart();
    st.valChart = LWC.createChart(div, {
      autoSize: true,
      layout: { background: { color: "transparent" }, textColor: themeColor("--muted", "#5b6672") },
      grid: { vertLines: { color: border }, horzLines: { color: border } },
      timeScale: { timeVisible: true, secondsVisible: false, borderColor: border },
    });
    const a = st.valChart.addLineSeries({
      color: themeColor("--accent", "#1caf83"),
      lineWidth: 2,
      priceLineVisible: false,
    });
    a.setData(ours);
    if (ok) {
      const b = st.valChart.addLineSeries({
        color: themeColor("--warn", "#96660a"),
        lineWidth: 1,
        lineStyle: 2,
        priceLineVisible: false,
        lastValueVisible: false,
      });
      b.setData(
        full0.chart.times
          .map((ti, i) => ({ time: Math.floor(ti / 1000), value: full0.chart.equity[i] }))
          .filter((d) => Number.isFinite(d.value) && Number.isFinite(d.time))
          .sort((x, y) => x.time - y.time)
      );
    } else {
      box.appendChild(h("div", "sg-hint", t("sgValCurveNoFull")));
    }
    st.valChart.timeScale().fitContent();
  }

  /// 三态渲染: 跑 → 转圈 / 完 → 结论文案 + 各段并排 + 离散度 + 曲线 / 错 → 原因。
  function renderValidateResult(job) {
    const box = $("sg-val-result");
    if (!box) return;
    destroyValChart();
    box.textContent = "";
    if (job.status === "running") {
      const row = h("div", "sg-job running");
      row.appendChild(h("span", "sg-spinner"));
      row.appendChild(h("span", null, t("sgValRunning")));
      box.appendChild(row);
      return;
    }
    if (job.status === "error") {
      box.appendChild(h("div", "sg-msg err", job.error || ""));
      return;
    }
    const v = job.validate;
    if (!v) return;
    // 结论: **服务端写好, 前端原样展示** —— 门槛分层与退化判定只有一处口径, 不两边各拼一份。
    box.appendChild(h("div", "sg-job-head sg-ok", t("sgVal") + " · " + v.window[0] + " → " + v.window[1]));
    box.appendChild(h("div", "sg-val-note", v.note || ""));
    if (v.threshold) {
      const key =
        v.threshold === "insufficient"
          ? "sgValThresholdInsufficient"
          : v.threshold === "ample"
            ? "sgValThresholdAmple"
            : "sgValThresholdMinimum";
      const line = h("div", "sg-val-chip" + (v.threshold === "insufficient" ? " warn" : ""), t(key));
      if (v.oos_days != null) {
        line.textContent += " · " + t("sgValOos") + " " + v.oos_days + t("sgValSegDays");
      }
      box.appendChild(line);
    }
    // 退化: true / false / null 三种都要如实 —— null 是"算不出来", 不能画成"没退化"。
    if (v.degraded === true) {
      box.appendChild(h("div", "sg-val-chip warn", t("sgValDegraded")));
    } else if (v.degraded === null || v.degraded === undefined) {
      box.appendChild(h("div", "sg-hint", t("sgValDegradedUnknown")));
    }
    // 各段并排。
    const grid = h("div", "sg-val-segs");
    for (const s of v.segments || []) {
      const card = h("div", "sg-val-seg");
      if (v.mode === "split" && s.index === v.oos_index) card.classList.add("sg-val-oos");
      card.appendChild(h("div", "sg-val-seg-label", s.label));
      card.appendChild(
        h("div", "sg-val-seg-range", s.start + " → " + s.end + " (" + s.days + t("sgValSegDays") + ")")
      );
      if (s.metrics) {
        card.appendChild(segMetricRow(s.metrics));
      } else {
        card.appendChild(h("div", "sg-msg err", s.reason || t("sgValSegFail")));
      }
      grid.appendChild(card);
    }
    box.appendChild(grid);
    renderSpread(box, v.segments || []);
    renderValChart(box, v, job.strategyLabel || (st.detailCtx && st.detailCtx.id) || "");
  }

  /// 用发起时的同一份参数重新发起(错误态一键重试)。
  async function rerunJob(job) {
    if (!st.detailCtx) return;
    const g = st.gen;
    syncBtBtn(true);
    const reply = await R.api("/api/backtest", { method: "POST", body: job.body });
    if (g !== st.gen) return;
    const next = {
      jobId: reply.job_id,
      status: "running",
      report: null,
      metrics: null,
      chart: null,
      logs: null,
      error: null,
      body: job.body,
      strategyLabel: job.strategyLabel || st.detailCtx.id,
    };
    st.jobs.set(st.detailCtx.id, next);
    renderJob(next);
    startPolling(st.detailCtx.id, next.jobId, g);
    syncBtBtn(false);
  }

  // ---------- N3-B: 改完自动评估影响(两次同窗口回测 + 实测指标解读) ----------
  //
  // 为什么由前端串三步而不是后端一个端点跑完: 回测是**作业**(同一策略只允许一个 running,
  // 带 progress / 5 分钟结果保留), 已有 `/api/backtest` 那一整套。在 AI 端点里再开一条同步
  // 回测路径 = 把并发规则抄第二遍, 而且"没配密钥"会让回测都跑不了 —— 那是两件独立的事。
  //
  // 窗口**固定**默认值(90 天 / 1h): 两次必须同窗口才可比。让用户两次各填一个窗口, 出来的差异
  // 就分不清是参数造成的还是窗口造成的 —— 那正是这次要消灭的"看起来有结论其实没有"。

  const ASSESS_DAYS = 90;
  const ASSESS_INTERVAL = "1h";
  /// 单次回测的等待上限(与后端 running 兜底 60 分钟同量级但更短: 到点**如实报超时**,
  /// 而不是无限转圈 —— 作业可能仍在跑, 用户可回下面的回测卡继续看)。
  const ASSESS_TIMEOUT_MS = 10 * 60 * 1000;

  /// 轮询一个回测作业直到 done/error, 返回 metrics。代际变了就地终止(绝不往旧视图写)。
  function waitForJob(jobId, g) {
    return new Promise((resolve, reject) => {
      const started = Date.now();
      const tick = async () => {
        if (g !== st.gen) {
          reject(new Error(t("sgAiAssessStale")));
          return;
        }
        let info;
        try {
          info = await R.api("/api/backtest/" + encodeURIComponent(jobId));
        } catch (err) {
          reject(err);
          return;
        }
        if (info.status === "done") {
          resolve(info.metrics || null);
          return;
        }
        if (info.status === "error") {
          reject(new Error(info.error || t("sgBtError")));
          return;
        }
        if (Date.now() - started > ASSESS_TIMEOUT_MS) {
          reject(new Error(t("sgAiAssessTimeout")));
          return;
        }
        setTimeout(() => {
          tick().catch(() => {});
        }, POLL_MS);
      };
      setTimeout(() => {
        tick().catch(() => {});
      }, POLL_MS);
    });
  }

  /// 跑一次回测并等结果(不登记台账、不画回测卡 —— 这是评估流程的私有作业, 与用户在回测卡
  /// 里点的那次互不覆盖。busy 冲突由服务端 409 如实带回)。
  async function assessRun(opts, params) {
    const reply = await R.api("/api/backtest", {
      method: "POST",
      body: {
        strategy: opts.id,
        pair: opts.pair,
        interval: ASSESS_INTERVAL,
        days: ASSESS_DAYS,
        params: params,
      },
    });
    return waitForJob(reply.job_id, opts.g);
  }

  /// N3-B 编排: 改前 → 改后 → AI 读两份实测指标。返回 `{ advice, base, new }`。
  /// 任一环节失败都**原样抛出**(带服务端中文原因), 由调用方贴在评估面板里。
  async function runParamAssess(opts) {
    if (!st.detailCtx) throw new Error(t("sgAiAssessStale"));
    // 同一策略已有回测在跑 → 说清楚而不是等一个必然 409 的请求。
    const running = st.jobs.get(opts.id);
    if (running && running.status === "running") throw new Error(t("sgAiAssessBusy"));
    opts.onStage && opts.onStage("base");
    const base = await assessRun(opts, opts.baseParams);
    if (opts.g !== st.gen) throw new Error(t("sgAiAssessStale"));
    opts.onStage && opts.onStage("new");
    const next = await assessRun(opts, opts.newParams);
    if (opts.g !== st.gen) throw new Error(t("sgAiAssessStale"));
    opts.onStage && opts.onStage("assess");
    const reply = await R.api(
      "/api/strategies/" + encodeURIComponent(opts.id) + "/ai-assess",
      {
        method: "POST",
        body: { changed: opts.changed, base_metrics: base, new_metrics: next },
      }
    );
    return { advice: reply.advice || "", base: base, new: next };
  }

  // ---------- 对外(外壳与其它子文件调用) ----------
  S.renderHistory = renderHistory;
  S.destroyBtChart = destroyBtChart;
  S.renderJob = renderJob;
  // 054: 生成弹窗要复用「指标卡 / 权益曲线 / 成本假设行」—— 不新写第二套渲染。
  S.renderMetricCards = renderMetricCards;
  S.renderBtChart = renderBtChart;
  S.renderCostLine = renderCostLine;
  S.renderVerifyResult = renderVerifyResult;
  // N3-B: 评估编排 —— 在 strategy_editor.js 的评估面板里被调用(跨文件走延迟别名)。
  S.runParamAssess = runParamAssess;
  S.onBacktest = onBacktest;
  S.onSweep = onSweep;
  S.onOptimize = onOptimize;
  S.onValidate = onValidate;
  S.destroyValChart = destroyValChart;
  S.syncBtBtn = syncBtBtn;
  S.startPolling = startPolling;
})();
