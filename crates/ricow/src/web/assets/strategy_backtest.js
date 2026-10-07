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
  /// `strategy` 可以是策略 id(普通回测)或 id + "#sweep"(寻优作业, P2-7)。
  function startPolling(strategy, jobId, g) {
    if (st.pollTimer) clearInterval(st.pollTimer);
    const stillHere = () =>
      !!st.detailCtx && (st.detailCtx.id === strategy || strategy === st.detailCtx.id + "#sweep");
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
      renderJob(job);
      if (info.status !== "running" && st.pollTimer) clearInterval(st.pollTimer);
    };
    st.pollTimer = setInterval(() => {
      tick().catch(() => {});
    }, POLL_MS);
  }

  /// 三态渲染: running 转圈 / done 指标卡+权益曲线+可折叠文本报告 / error 原因+重试。
  /// 寻优作业(kind==="sweep")另走对比表渲染。
  function renderJob(job) {
    const box = $("sg-bt-result");
    destroyBtChart();
    box.innerHTML = "";
    if (job.kind === "sweep") {
      renderSweepResult(job);
      return;
    }
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
        renderMetricCards(box, m);
        pushHistory(m, job.strategyLabel || (st.detailCtx && st.detailCtx.id) || "");
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
      [t("sgSharpe"), m.sharpe != null ? m.sharpe.toFixed(2) : "—", "flat", ""],
    ];
    for (const [label, value, cls, sub] of cards) {
      const card = h("div", "sg-mcard");
      card.appendChild(h("span", "sg-mcard-label", label));
      const v = h("span", "sg-mcard-value" + (cls ? " " + cls : ""), value);
      card.appendChild(v);
      if (sub) card.appendChild(h("span", "sg-mcard-sub", sub));
      grid.appendChild(card);
    }
    box.appendChild(grid);
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
      legend.appendChild(h("span", "lg " + cls, t(key)));
    }
    wrap.appendChild(legend);
    const div = h("div", "sg-bt-chart");
    wrap.appendChild(div);
    box.appendChild(wrap);
    const border = themeColor("--border", "#2b333d");
    const LWC = window.LightweightCharts;
    st.btChart = LWC.createChart(div, {
      autoSize: true,
      layout: { background: { color: "transparent" }, textColor: themeColor("--muted", "#9aa7b4") },
      grid: { vertLines: { color: border }, horzLines: { color: border } },
      timeScale: { timeVisible: true, secondsVisible: false, borderColor: border },
    });
    const sec = (ms) => Math.floor(ms / 1000);
    // 价格线(muted 细线)。
    const price = st.btChart.addLineSeries({
      color: themeColor("--muted", "#9aa7b4"),
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
    // 权益线(accent 粗线)。
    const eq = st.btChart.addLineSeries({
      color: themeColor("--accent", "#4dd4ac"),
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
        color: themeColor("--warn", "#e3b341"),
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
    const buyColor = themeColor("--error", "#f85149");
    const sellColor = themeColor("--accent", "#4dd4ac");
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
    hr.appendChild(h("th", null, t("sgClosedThPnl")));
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
    box.appendChild(h("div", "sg-closed-sub", note));
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

  function fmtPct(v) {
    if (v == null || !Number.isFinite(v)) return "—";
    return (v > 0 ? "+" : "") + v.toFixed(2) + "%";
  }

  function fmtNum(v) {
    if (v == null || !Number.isFinite(v)) return "—";
    const abs = Math.abs(v);
    const digits = abs >= 100 ? 2 : abs >= 1 ? 4 : 6;
    return (v > 0 ? "+" : "") + v.toFixed(digits).replace(/\.?0+$/, "");
  }

  /// 毫秒时间戳 → 展示串 (平仓明细用)。会话列表的秒级口径走 `R.formatTime`, 两者不可混
  /// (与 chat.js 的 `formatStamp` 同款格式, 但各视图各持一份 —— 那两份本来就不同口径)。
  function fmtMs(ms) {
    if (!Number.isFinite(ms)) return "—";
    const locale = R.lang === "en" ? "en-US" : "zh-CN";
    return new Date(ms).toLocaleString(locale, {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
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
      "夏普 " + (m.sharpe != null ? m.sharpe.toFixed(2) : "无"),
      "胜率 " + fmtPct(m.win_rate),
      "成交 " + (m.total_trades ?? 0) + " 笔",
    ];
    const ta = $("sg-ai-instr");
    ta.value = parts.join(", ") + "。请基于这份结果改进策略(例如先提高收益/降低回撤/减少无效交易)。";
    ta.scrollIntoView({ block: "center" });
    ta.focus();
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
        r.sharpe != null ? r.sharpe.toFixed(2) : "—",
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
          m.sharpe != null ? m.sharpe.toFixed(2) : "—",
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

  // ---------- 对外(外壳与其它子文件调用) ----------
  S.renderHistory = renderHistory;
  S.destroyBtChart = destroyBtChart;
  S.renderJob = renderJob;
  S.onBacktest = onBacktest;
  S.onSweep = onSweep;
  S.syncBtBtn = syncBtBtn;
  S.startPolling = startPolling;
})();
