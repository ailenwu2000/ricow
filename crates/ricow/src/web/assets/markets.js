// Ricow Web 前端「市场」视图 (032 US2): 交易对视野列表 + 标的详情(现价 / 买卖盘 / K 线)。
// 注册为 `R.views.markets`; 只读行情全部走 R.api 的 `/api/markets*`(免 key 公共行情);
// 视野开关复用 US1 的 POST /api/config/keys(与终端同一份 ricow.toml)。
//
// 过期响应处理: 视图级"代际令牌" `gen` —— 每次 activate / deactivate 自增, 异步响应回来先
// 比对自己捕获的代际, 不一致即丢弃(离开视图 / 切换标的后旧响应不会再碰 DOM)。
//
// 安全约束: 不写任何浏览器持久存储; 全部端点在 token 门禁之后。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  // ---------- 双语文案(并入公共字典, 静态标签走 data-zh/data-en) ----------
  Object.assign(R.TEXT.zh, {
    mkTitle: "市场",
    mkShowAll: "显示全部",
    mkSearchPh: "搜索代码…",
    mkViewFiltered: "过滤视野",
    mkViewAll: "全部视野",
    mkLoading: "加载中…",
    mkLoadFailed: "加载失败: ",
    mkToggleFailed: "视野切换失败: ",
    mkRetry: "重试",
    mkEmpty: "无",
    mkBack: "‹ 返回市场",
    mkLastPrice: "现价(买卖盘中间价)",
    mkNoBook: "暂无盘口",
    mkBookTitle: "买卖盘(前 20 档)",
    mkBids: "买盘",
    mkAsks: "卖盘",
    mkPriceCol: "价格",
    mkSizeCol: "数量",
    mkChartTitle: "K 线",
    mkBestBid: "买一",
    mkBestAsk: "卖一",
    mkChartEmpty: "暂无 K 线",
    mkLegVol: "成交量",
    // 040 实时行情状态标识 (FR-9 / FR-10): 三态 + 按路如实说明, 绝不静默假装实时。
    mkLive: "实时",
    mkLiveConnecting: "连接中…",
    mkLiveOffline: "实时已断开",
    mkLivePartial: "部分无实时",
    mkLiveStale: "秒未更新",
    mkScopeKline: "K 线",
    mkScopeDepth: "盘口",
    mkRetry: "重试",
  });
  Object.assign(R.TEXT.en, {
    mkTitle: "Markets",
    mkShowAll: "Show all",
    mkSearchPh: "Search symbols…",
    mkViewFiltered: "Filtered view",
    mkViewAll: "All pairs",
    mkLoading: "Loading…",
    mkLoadFailed: "Load failed: ",
    mkToggleFailed: "Failed to switch view: ",
    mkRetry: "Retry",
    mkEmpty: "None",
    mkBack: "‹ Back to markets",
    mkLastPrice: "Last price (order-book mid)",
    mkNoBook: "Order book unavailable",
    mkBookTitle: "Order book (top 20)",
    mkBids: "Bids",
    mkAsks: "Asks",
    mkPriceCol: "Price",
    mkSizeCol: "Size",
    mkChartTitle: "Candlesticks",
    mkBestBid: "Best bid",
    mkBestAsk: "Best ask",
    mkChartEmpty: "No candlesticks",
    mkLegVol: "Vol",
    mkLive: "Live",
    mkLiveConnecting: "Connecting…",
    mkLiveOffline: "Live disconnected",
    mkLivePartial: "Partial live",
    mkLiveStale: "s since update",
    mkScopeKline: "Candles",
    mkScopeDepth: "Order book",
    mkRetry: "Retry",
  });

  // K 线周期按钮(默认 1h; 不含 1m/5m —— 视图契约只要 15m/1h/4h/1d)。
  const INTERVALS = ["15m", "1h", "4h", "1d"];
  const DEFAULT_INTERVAL = "1h";
  const KLINE_LIMIT = 200;
  const SEARCH_DEBOUNCE_MS = 250;

  // 039: K 线指标 —— 固定均线窗口 + 成交量副图。指标全在前端由 klines 现算(零后端改动、零新依赖)。
  const MA_WINDOWS = [7, 25, 99];
  /// 均线取色用的 CSS 变量名 (颜色本体定义在 style.css 的各主题块里)。
  const MA_VARS = ["--ma-7", "--ma-25", "--ma-99"];
  /// 成交量副图占图表底部的高度比例 (与主图共享时间轴, 但用独立价格轴 → 不需要同步代码)。
  const VOL_PANE_MARGIN = 0.82;

  // 040: 实时流。
  /// 服务端就绪窗口是 12s; 页面侧"多久没收到任何一帧就算陈旧"取 15s ——
  /// 比服务端宽一点, 免得服务端刚报完"无数据"页面又自己造一个不同的说法。
  const LIVE_STALE_MS = 15000;
  /// 状态标识的自刷新节奏(只更新那个 chip, 不动图表)。
  const LIVE_TICK_MS = 2000;

  let mounted = false;
  // 代际令牌(见文件头注释): 每次激活 / 离开自增; 回调捕获本次值, 过期响应直接丢弃。
  let gen = 0;
  let searchTimer = 0;
  // 当前 LightweightCharts 实例(离开视图 / 重渲染前 remove 掉, 连同自适应观察器一起释放)。
  let chart = null;
  // 图表请求令牌: 连点周期按钮时代际不变(仍在同一详情态), 用它丢弃先出发后到的旧周期响应。
  let chartReq = 0;
  // 列表态缓存(视野开关 / 检索词 / 最近数据); 从列表行点入详情时顺带记下该行所属市场,
  // 因为同符号可能现货/合约都存在(如全量视野里的 BTCUSDT), hash 只带符号无法区分。
  const listState = { q: "", showAll: false, data: null, marketOf: null };
  // 详情态当前上下文(周期按钮回调读它)。
  let detailCtx = null;

  // 040 实时流句柄与状态。`streamCtx` = 当前订阅目标(与 `detailCtx` 分开: 周期切换时
  // 图表在重取, 但订阅目标已经变了, 两者不是一回事)。
  let stream = null;
  let streamCtx = null;
  /// 连接态: connecting / open / failed(`failed` = R.sse 退避重试已放弃)。
  let connState = "connecting";
  /// 最近一次**收到数据帧**的时刻(本地钟, ms) —— 陈旧判据的唯一依据。
  let lastFrameAt = 0;
  let liveTimer = 0;
  /// 本次 openStream 之后是否已收到过 hello(用于区分"首屏"与"重连")。
  let helloSeen = false;
  /// 按路状态: pending(还没数据) / live / error(附原因)。
  const liveState = { kline: "pending", depth: "pending", klineMsg: "", depthMsg: "" };

  const $ = (id) => document.getElementById(id);

  /// 建元素的小工具: 文本一律走 textContent(符号即使来自服务端也不拼 HTML)。
  function h(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  /// 双语属性串(拼 innerHTML 时用, 与 settings.js 同款手法)。
  function bilingual(key) {
    return ' data-zh="' + R.TEXT.zh[key] + '" data-en="' + R.TEXT.en[key] + '"';
  }

  // ---------- 一次性挂载(只建 DOM 与事件; 反复激活只重新取数) ----------
  function mount() {
    if (mounted) return;
    const host = $("view-markets");
    host.innerHTML =
      '<div class="mk-wrap">' +
      // ---- 列表态 ----
      '<div id="mk-listview" class="mk-listview">' +
      '<div class="mk-head"><h2 class="mk-title"' + bilingual("mkTitle") + ">" +
      R.TEXT.zh.mkTitle +
      "</h2>" +
      '<div class="mk-toolbar"><label class="mk-check"><input id="mk-show-all" type="checkbox" />' +
      "<span" + bilingual("mkShowAll") + ">" + R.TEXT.zh.mkShowAll + "</span></label>" +
      '<input id="mk-search" class="mk-search" type="text" autocomplete="off" spellcheck="false"' +
      ' data-zh-placeholder="' + R.TEXT.zh.mkSearchPh + '" data-en-placeholder="' +
      R.TEXT.en.mkSearchPh + '" /></div>' +
      '<div id="mk-scope" class="mk-scope" aria-live="polite"></div>' +
      '<div id="mk-tool-msg" class="mk-tool-msg" aria-live="polite"></div></div>' +
      '<div id="mk-list-body" class="mk-columns"></div></div>' +
      // ---- 详情态 ----
      '<div id="mk-detail" class="mk-detail" hidden>' +
      '<div class="mk-crumbs"><a href="javascript:void(0)" id="mk-back"' + bilingual("mkBack") +
      ">" + R.TEXT.zh.mkBack +
      '</a><span id="mk-symbol" class="mk-symbol"></span>' +
      // 040: 实时状态标识 (FR-9) + 按路说明 (FR-10)。文案由 renderLive() 用 DOM API 填。
      '<span id="mk-live" class="mk-live" hidden></span></div>' +
      '<div id="mk-live-note" class="mk-live-note" hidden></div>' +
      '<div class="mk-detail-grid">' +
      '<section class="mk-card"><div class="mk-card-head"><span' + bilingual("mkLastPrice") +
      ">" + R.TEXT.zh.mkLastPrice + "</span></div>" +
      '<div id="mk-price-body" class="mk-card-body"></div></section>' +
      '<section class="mk-card"><div class="mk-card-head"><span' + bilingual("mkBookTitle") + ">" +
      R.TEXT.zh.mkBookTitle + "</span></div>" +
      '<div id="mk-book-body" class="mk-card-body"></div></section>' +
      '<section class="mk-card mk-chart-card"><div class="mk-card-head"><span' +
      bilingual("mkChartTitle") + ">" + R.TEXT.zh.mkChartTitle + "</span>" +
      '<span id="mk-intervals" class="mk-intervals"></span></div>' +
      // 039: 图例 (均线 × 3 + 成交量) —— 颜色与线一致, 由 CSS 变量给出。
      // 均线标签是语言无关的("MA7"), 走纯文本; 只有"成交量/Vol"需要双语。
      '<div class="mk-chart-legend">' +
      MA_WINDOWS.map((w) => '<span class="lg lg-ma' + w + '">MA' + w + "</span>").join("") +
      '<span class="lg lg-vol"' + bilingual("mkLegVol") + ">" + t("mkLegVol") + "</span>" +
      "</div>" +
      '<div id="mk-chart-box" class="mk-chart-box"></div></section>' +
      "</div></div></div>";

    $("mk-show-all").addEventListener("change", onToggleShowAll);
    $("mk-search").addEventListener("input", onSearchInput);
    $("mk-back").addEventListener("click", () => R.navigate("markets"));
    buildIntervalButtons();
    mounted = true;
  }

  // ---------- 块级状态(loading / 失败+重试 / 内容, 三块互相独立, FR-014) ----------

  function blockLoading(bodyEl) {
    bodyEl.innerHTML = "";
    bodyEl.appendChild(h("div", "mk-loading", t("mkLoading")));
  }

  function blockError(bodyEl, err, retryFn) {
    bodyEl.innerHTML = "";
    const box = h("div", "mk-error-box");
    box.appendChild(h("div", "mk-error", t("mkLoadFailed") + ((err && err.message) || "")));
    const btn = h("button", "mk-retry", t("mkRetry"));
    btn.type = "button";
    btn.addEventListener("click", retryFn);
    box.appendChild(btn);
    bodyEl.appendChild(box);
  }

  // ---------- 列表态 ----------

  /// 进入列表: 先读视野开关(本地配置, 决定勾选态), 再取列表。
  async function enterList(g) {
    $("mk-search").value = listState.q;
    setToolMsg("", false);
    blockLoading($("mk-list-body"));
    try {
      const cfg = await R.api("/api/config/keys");
      if (g !== gen) return; // 过期响应: 已离开列表, 丢弃
      listState.showAll = !!(cfg.market && cfg.market.show_all_pairs);
      $("mk-show-all").checked = listState.showAll;
      await fetchList(g);
    } catch (err) {
      if (g !== gen) return;
      blockError($("mk-list-body"), err, () => enterList(g));
    }
  }

  /// 只取列表(搜索 / 切换视野后调用; 视野已持久化在配置里, 不必再带 all)。
  async function fetchList(g) {
    blockLoading($("mk-list-body"));
    let path = "/api/markets";
    if (listState.q) path += "?q=" + encodeURIComponent(listState.q);
    try {
      const data = await R.api(path);
      if (g !== gen) return;
      listState.data = data;
      renderList();
    } catch (err) {
      if (g !== gen) return;
      blockError($("mk-list-body"), err, () => fetchList(g));
    }
  }

  function renderScope(d) {
    const scope = d.filtered ? t("mkViewFiltered") : t("mkViewAll");
    $("mk-scope").textContent =
      scope + " · " + t("spot") + " " + d.spot.length + " · " + t("futures") + " " + d.futures.length;
  }

  function renderList() {
    const d = listState.data;
    if (!d) return;
    renderScope(d);
    const body = $("mk-list-body");
    body.innerHTML = "";
    body.appendChild(renderGroup("mk-spot", t("spot"), d.spot, "spot"));
    body.appendChild(renderGroup("mk-futures", t("futures"), d.futures, "futures"));
  }

  /// 一组(现货 / 合约)可点行; 点击记下所属市场后跳详情。
  function renderGroup(id, label, symbols, market) {
    const sec = h("section", "mk-col");
    const head = h("div", "mk-col-head");
    head.appendChild(h("span", null, label));
    head.appendChild(h("span", "mk-count", String(symbols.length)));
    sec.appendChild(head);
    const rows = h("div", "mk-symrows");
    if (!symbols.length) {
      rows.appendChild(h("div", "mk-empty", t("mkEmpty")));
    } else {
      for (const symbol of symbols) {
        const row = h("button", "mk-row", symbol);
        row.type = "button";
        row.addEventListener("click", () => {
          listState.marketOf = { symbol: symbol, market: market };
          R.navigate("markets/" + encodeURIComponent(symbol));
        });
        rows.appendChild(row);
      }
    }
    sec.appendChild(rows);
    sec.id = id;
    return sec;
  }

  /// 搜索框: 停笔 250ms 才发服务端子串过滤(两组同步由后端一次返回)。
  function onSearchInput() {
    const g = gen;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      listState.q = $("mk-search").value.trim();
      fetchList(g);
    }, SEARCH_DEBOUNCE_MS);
  }

  /// "显示全部": change 即写盘(POST US1 端点); 成功重取, 失败回滚勾选(与设置页同款)。
  async function onToggleShowAll() {
    const g = gen;
    const cb = $("mk-show-all");
    const wanted = cb.checked;
    setToolMsg("", false);
    try {
      await R.api("/api/config/keys", {
        method: "POST",
        body: { updates: [{ section: "market", key: "show_all_pairs", value_bool: wanted }] },
      });
      listState.showAll = wanted;
      await fetchList(g);
    } catch (err) {
      if (g !== gen) return;
      cb.checked = !wanted; // 回滚
      setToolMsg(t("mkToggleFailed") + ((err && err.message) || ""), true);
    }
  }

  function setToolMsg(text, isErr) {
    const msg = $("mk-tool-msg");
    msg.textContent = text || "";
    msg.classList.toggle("err", isErr === true);
  }

  // ---------- 详情态 ----------

  /// 解析标的所属市场: 行点击带来的归属 > 已载列表精确匹配 > 全量子串检索定位(直达/刷新链接)。
  /// 过期返回 null(调用方丢弃后续); 同符号两组都在时现货优先。
  async function resolveMarket(symbol, g) {
    if (listState.marketOf && listState.marketOf.symbol === symbol) {
      return listState.marketOf.market;
    }
    const d = listState.data;
    if (d) {
      if (d.spot.indexOf(symbol) >= 0) return "spot";
      if (d.futures.indexOf(symbol) >= 0) return "futures";
    }
    const hit = await R.api("/api/markets?all=1&q=" + encodeURIComponent(symbol));
    if (g !== gen) return null;
    if (hit.spot.indexOf(symbol) >= 0) return "spot";
    if (hit.futures.indexOf(symbol) >= 0) return "futures";
    return "spot"; // 两处都查不到: 现货优先, 各块如实呈现失败与重试
  }

  async function enterDetail(symbol, g) {
    $("mk-symbol").textContent = symbol;
    detailCtx = { symbol: symbol, market: null, interval: DEFAULT_INTERVAL };
    setIntervalActive(DEFAULT_INTERVAL);
    destroyChart();
    const priceBody = $("mk-price-body");
    const bookBody = $("mk-book-body");
    blockLoading(priceBody);
    blockLoading(bookBody);
    blockLoading($("mk-chart-box"));

    let market;
    try {
      market = await resolveMarket(symbol, g);
    } catch (err) {
      if (g !== gen) return;
      // 归属解析失败: 三块统一给失败态 + 重试(不猜市场, 不用伪数据)。
      blockError(priceBody, err, () => loadDetail(g, symbol));
      blockError(bookBody, err, () => loadDetail(g, symbol));
      blockError($("mk-chart-box"), err, () => loadDetail(g, symbol));
      return;
    }
    if (g !== gen || !market) return;
    detailCtx.market = market;
    // 现价单独取一次 depth=1 快照作首屏(实时流的第一帧盘口 ~100ms 内会接管它)。
    loadPrice(g, symbol, market);
    // 040: 盘口与 K 线的首屏快照由 openStream 内部取(REST, 与 SSE 无关 ——
    // 实时流连不上时这三块仍能看, 只是状态标识会说"已断开")。
    openStream(symbol, market, DEFAULT_INTERVAL);
  }

  /// 重试入口(解析失败后): 重跑整个详情装配。
  function loadDetail(g, symbol) {
    enterDetail(symbol, g);
  }

  /// 现价: 独立 depth=1 请求, mid = (best bid + best ask) / 2; 任一侧无盘口给 "--"。
  async function loadPrice(g, symbol, market) {
    const body = $("mk-price-body");
    blockLoading(body);
    try {
      const book = await R.api(
        "/api/markets/" + encodeURIComponent(symbol) + "/orderbook?market=" + market + "&depth=1"
      );
      if (g !== gen) return;
      renderPrice(body, book);
    } catch (err) {
      if (g !== gen) return;
      blockError(body, err, () => loadPrice(g, symbol, market));
    }
  }

  function renderPrice(body, book) {
    const bid = book.bids && book.bids[0];
    const ask = book.asks && book.asks[0];
    body.innerHTML = "";
    const mid =
      bid && ask ? formatNum((Number(bid.price) + Number(ask.price)) / 2) : null;
    body.appendChild(h("div", "mk-mid", mid === null ? "--" : mid));
    const sub = h("div", "mk-mid-sub");
    sub.appendChild(h("span", "mk-bid", t("mkBestBid") + " " + (bid ? bid.price : "--")));
    sub.appendChild(h("span", "mk-ask", t("mkBestAsk") + " " + (ask ? ask.price : "--")));
    body.appendChild(sub);
    if (mid === null) body.appendChild(h("div", "mk-muted", t("mkNoBook")));
  }

  /// 买卖盘: depth=20 独立请求, bids / asks 两张小表(各取前 20 档)。
  async function loadBook(g, symbol, market) {
    const body = $("mk-book-body");
    blockLoading(body);
    try {
      const book = await R.api(
        "/api/markets/" + encodeURIComponent(symbol) + "/orderbook?market=" + market + "&depth=20"
      );
      if (g !== gen) return;
      renderBook(body, book);
    } catch (err) {
      if (g !== gen) return;
      blockError(body, err, () => loadBook(g, symbol, market));
    }
  }

  function bookTable(titleText, levels, cls) {
    const wrap = h("div", "mk-book-col " + cls);
    wrap.appendChild(h("div", "mk-book-subhead", titleText));
    const tbl = h("table", "mk-book-table");
    const thead = h("thead");
    const hr = h("tr");
    hr.appendChild(h("th", null, t("mkPriceCol")));
    hr.appendChild(h("th", null, t("mkSizeCol")));
    thead.appendChild(hr);
    tbl.appendChild(thead);
    const tbody = h("tbody");
    if (!levels.length) {
      const tr = h("tr");
      const td = h("td", "mk-muted", t("mkEmpty"));
      td.colSpan = 2;
      tr.appendChild(td);
      tbody.appendChild(tr);
    } else {
      for (const lv of levels.slice(0, 20)) {
        const tr = h("tr");
        tr.appendChild(h("td", "num", lv.price));
        tr.appendChild(h("td", "num", lv.size));
        tbody.appendChild(tr);
      }
    }
    tbl.appendChild(tbody);
    wrap.appendChild(tbl);
    return wrap;
  }

  function renderBook(body, book) {
    body.innerHTML = "";
    const grid = h("div", "mk-book-grid");
    grid.appendChild(bookTable(t("mkBids"), book.bids || [], "is-bid"));
    grid.appendChild(bookTable(t("mkAsks"), book.asks || [], "is-ask"));
    body.appendChild(grid);
  }

  // ---------- 040 实时行情 (SSE) ----------
  //
  // 一条 SSE 同时推 K 线与盘口; 上游由服务端按 (市场,标的,周期) 共享。页面只负责:
  // ① 增量更新图表/盘口; ② **如实**呈现连接状态与"哪一路没有数据"(FR-9 / FR-10);
  // ③ 连接(重)建立时用 REST 快照重新对齐 —— 行情没有"补发"语义, 断线期间的变动不可能补发,
  //    重放旧帧只会把图拉回过去, 所以不做事件续传(FR-11)。

  function openStream(symbol, market, interval) {
    closeStream();
    streamCtx = { symbol: symbol, market: market, interval: interval };
    connState = "connecting";
    lastFrameAt = 0;
    liveState.kline = "pending";
    liveState.depth = "pending";
    liveState.klineMsg = "";
    liveState.depthMsg = "";
    helloSeen = false;
    renderLive();
    // 首屏快照立刻取(不依赖 SSE 是否连上)。
    resyncFromRest();
    const g = gen;
    const url =
      "/api/markets/" + encodeURIComponent(symbol) + "/stream?market=" + market +
      "&interval=" + encodeURIComponent(interval) + "&token=" + encodeURIComponent(R.TOKEN);
    stream = R.sse(url, {
      onopen: () => {
        if (g !== gen || !streamCtx || streamCtx.symbol !== symbol) return;
        connState = "open";
        renderLive();
      },
      onmessage: (ev) => {
        if (g !== gen || !streamCtx || streamCtx.symbol !== symbol) return;
        onFrame(ev.data);
      },
      onfail: () => {
        // 退避重试已放弃: 明说断了, 并给一个手动重试 —— 不静默降级成轮询(那会让页面
        // 看起来仍在实时)。屏幕上已有的快照保留, 但状态标识不再说"实时"。
        if (g !== gen || !streamCtx || streamCtx.symbol !== symbol) return;
        connState = "failed";
        renderLive();
      },
    });
    liveTimer = setInterval(renderLive, LIVE_TICK_MS);
  }

  function closeStream() {
    if (liveTimer) {
      clearInterval(liveTimer);
      liveTimer = 0;
    }
    if (stream) {
      try {
        stream.close();
      } catch (_) {
        // R.sse 的 close 是幂等的; 异常不影响后续状态清理。
      }
      stream = null;
    }
    streamCtx = null;
    lastFrameAt = 0;
    renderLive();
  }

  /// 一帧到 → 按 `type` 分派(与会话流同一套约定)。
  function onFrame(raw) {
    let f;
    try {
      f = JSON.parse(raw);
    } catch (_) {
      return; // 非 JSON 帧: 丢弃(不入 DOM)。
    }
    if (!f || typeof f.type !== "string") return;
    if (f.type === "hello") {
      // 首次 hello = 首屏那一次快照刚取过, 不重复拉; **重连**后的 hello 才重新对齐
      // (补齐断线期间的空档 —— 不做事件续传, 见 FR-11)。
      if (helloSeen) resyncFromRest();
      helloSeen = true;
      return;
    }
    if (f.type === "kline") {
      lastFrameAt = Date.now();
      liveState.kline = "live";
      applyKlineFrame(f);
      renderLive();
      return;
    }
    if (f.type === "depth") {
      lastFrameAt = Date.now();
      liveState.depth = "live";
      applyDepthFrame(f);
      renderLive();
      return;
    }
    if (f.type === "error") {
      // 按路如实标记(FR-10): 某一路没数据不该连累另一路。
      if (f.scope === "kline") {
        liveState.kline = "error";
        liveState.klineMsg = String(f.msg || "");
      } else if (f.scope === "depth") {
        liveState.depth = "error";
        liveState.depthMsg = String(f.msg || "");
      }
      renderLive();
    }
  }

  /// 连接(重)建立后按 REST 重新对齐两块快照。此窗口内到达的少数帧会随后续帧覆盖。
  function resyncFromRest() {
    if (!streamCtx) return;
    const ctx = streamCtx;
    const g = gen;
    loadBook(g, ctx.symbol, ctx.market);
    loadChart(g, ctx.symbol, ctx.market, ctx.interval);
  }

  /// K 线帧 → 原地更新最后一根(同一个 `t`)或长出新的一根。
  function applyKlineFrame(f) {
    const bar = {
      time: Math.floor(Number(f.t) / 1000),
      open: Number(f.o),
      high: Number(f.h),
      low: Number(f.l),
      close: Number(f.c),
      volume: Number(f.v),
    };
    if (!Number.isFinite(bar.time) || !Number.isFinite(bar.close)) return;
    const last = chartData.length ? chartData[chartData.length - 1] : null;
    if (last && bar.time < last.time) {
      // 时间倒退: LightweightCharts 的 update 要求时间单调, 塞进去会炸掉整张图。
      // 丢弃这一帧(不猜、不插), 图表最多晚一拍。
      return;
    }
    if (!last || bar.time > last.time) chartData.push(bar);
    else chartData[chartData.length - 1] = bar;
    if (!chartSeries) return;
    chartSeries.candle.update(bar);
    if (Number.isFinite(bar.volume)) {
      chartSeries.vol.update({
        time: bar.time,
        value: bar.volume,
        color: bar.close >= bar.open ? chartSeries.up : chartSeries.down,
      });
    }
    // 均线尾点: 值只取决于"最近 w 个收盘价", 故只重算最后一点, 不整列重算。
    for (let i = 0; i < MA_WINDOWS.length; i++) {
      const v = maValue(chartData, MA_WINDOWS[i]);
      if (v !== null && chartSeries.ma[i]) {
        chartSeries.ma[i].update({ time: bar.time, value: v });
      }
    }
  }

  /// 盘口帧 → 整体替换(上游已是**累计**语义, 不需要自己合并)。
  function applyDepthFrame(f) {
    const body = $("mk-book-body");
    const priceBody = $("mk-price-body");
    if (!body || !priceBody) return;
    const book = { bids: f.bids || [], asks: f.asks || [] };
    renderBook(body, book);
    renderPrice(priceBody, book);
  }

  /// 状态标识 + 按路说明。三态 + 陈旧提示, 绝不在断流时说"实时"。
  function renderLive() {
    const el = $("mk-live");
    const noteEl = $("mk-live-note");
    if (!el) return;
    el.textContent = "";
    if (!streamCtx) {
      el.hidden = true;
      if (noteEl) noteEl.hidden = true;
      return;
    }
    el.hidden = false;
    const errs = [];
    if (liveState.kline === "error") {
      errs.push(t("mkScopeKline") + ": " + (liveState.klineMsg || "—"));
    }
    if (liveState.depth === "error") {
      errs.push(t("mkScopeDepth") + ": " + (liveState.depthMsg || "—"));
    }
    const age = lastFrameAt ? Date.now() - lastFrameAt : null;
    const stale = connState === "open" && age !== null && age > LIVE_STALE_MS;
    let state;
    let label;
    if (connState === "failed") {
      state = "off";
      label = t("mkLiveOffline");
    } else if (liveState.kline !== "live" && liveState.depth !== "live" && !errs.length) {
      state = "wait";
      label = t("mkLiveConnecting");
    } else if (errs.length) {
      state = "warn";
      label = t("mkLivePartial");
    } else if (stale) {
      state = "warn";
      label = t("mkLive") + " · " + Math.round(age / 1000) + t("mkLiveStale");
    } else {
      state = "live";
      label = t("mkLive") + (lastFrameAt ? " · " + clockText(lastFrameAt) : "");
    }
    el.dataset.state = state;
    el.appendChild(h("span", "mk-live-dot"));
    el.appendChild(h("span", null, label));
    if (connState === "failed") {
      const btn = h("button", "mk-live-retry", t("mkRetry"));
      btn.type = "button";
      btn.addEventListener("click", () => {
        if (streamCtx) openStream(streamCtx.symbol, streamCtx.market, streamCtx.interval);
      });
      el.appendChild(btn);
    }
    el.title = errs.join(" · ") || label;
    if (noteEl) {
      // 按路说明直接可见(不藏在 hover 里): 用户不必猜"为什么这半页不动了"。
      const txt = errs.join(" · ");
      noteEl.hidden = !txt;
      noteEl.textContent = txt;
    }
  }

  function clockText(ms) {
    const d = new Date(ms);
    const p = (n) => (n < 10 ? "0" + n : String(n));
    return p(d.getHours()) + ":" + p(d.getMinutes()) + ":" + p(d.getSeconds());
  }

  // ---------- K 线 ----------

  function buildIntervalButtons() {
    const box = $("mk-intervals");
    for (const iv of INTERVALS) {
      const btn = h("button", "mk-iv", iv);
      btn.type = "button";
      btn.dataset.iv = iv;
      btn.addEventListener("click", () => {
        // 仅重取图表一块; 详情上下文此刻必然有效(按钮只在详情态可见可点)。
        const g = gen;
        const ctx = detailCtx;
        if (!ctx) return;
        setIntervalActive(iv);
        // 040: 周期变了 → K 线上游的订阅键也变了(键含周期), 必须换一条流;
        // 首屏快照由 openStream 内部重取, 这里不再单独 loadChart(否则白拉两次)。
        openStream(ctx.symbol, ctx.market, iv);
      });
      box.appendChild(btn);
    }
  }

  function setIntervalActive(iv) {
    for (const btn of $("mk-intervals").querySelectorAll(".mk-iv")) {
      btn.classList.toggle("active", btn.dataset.iv === iv);
    }
  }

  /// K 线: 默认 1h / 200 根; open_time(ISO8601)→ unix 秒喂 LightweightCharts v4。
  async function loadChart(g, symbol, market, interval) {
    const box = $("mk-chart-box");
    if (detailCtx) detailCtx.interval = interval;
    const req = (chartReq += 1); // 本次取图的请求号; 后发先至由它作废
    destroyChart();
    blockLoading(box);
    try {
      const rows = await R.api(
        "/api/markets/" + encodeURIComponent(symbol) +
          "/klines?market=" + market + "&interval=" + interval + "&limit=" + KLINE_LIMIT
      );
      if (g !== gen || req !== chartReq) return;
      renderChart(box, rows);
    } catch (err) {
      if (g !== gen || req !== chartReq) return;
      blockError(box, err, () => loadChart(g, symbol, market, interval));
    }
  }

  /// 034: 图表配色跟随当前主题 —— 现场从 CSS 变量取值(变量随 `<html data-theme>` 切换)。
  function themeColor(name, fallback) {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  }

  // 040: 图上的数据与序列句柄提到模块级 —— 实时帧要**就地更新**最后一根,
  // 而不是整张图重建(重建会丢缩放位置与十字光标)。
  /// 当前图上的 K 线(含实时帧更新的部分), 时间升序。
  let chartData = [];
  /// 图表序列句柄: 实时帧按它做 `update`。
  let chartSeries = null;

  /// REST K 线行 → 图表 bar。**时间统一 unix 秒**: 实时帧给的是 epoch 毫秒, 这里两条路径
  /// 都折成秒, 才能保证实时帧落在同一根 K 线上(`update` 而不是多长一根)。
  function rowsToBars(rows) {
    return rows
      .map((k) => ({
        time: Math.floor(new Date(k.open_time).getTime() / 1000),
        open: Number(k.open),
        high: Number(k.high),
        low: Number(k.low),
        close: Number(k.close),
        volume: Number(k.volume),
      }))
      .filter((d) => Number.isFinite(d.time) && Number.isFinite(d.close))
      .sort((a, b) => a.time - b.time);
  }

  /// 窗口 `w` 的均线值(末尾 w 个收盘价均值); 不足 w 根回 `null` —— 不画、不补 0、不用前值冒充。
  function maValue(bars, w) {
    if (bars.length < w) return null;
    let sum = 0;
    for (let i = bars.length - w; i < bars.length; i++) sum += bars[i].close;
    return sum / w;
  }

  /// 全量均线(首屏用)。数据不足窗口长度的前 w-1 根自然缺失(空点 → 断线)。
  function maSeriesData(bars, w) {
    const out = [];
    let sum = 0;
    for (let k = 0; k < bars.length; k++) {
      sum += bars[k].close;
      if (k >= w) sum -= bars[k - w].close;
      if (k >= w - 1) out.push({ time: bars[k].time, value: sum / w });
    }
    return out;
  }

  function renderChart(box, rows) {
    renderBars(box, rowsToBars(rows));
  }

  /// 画一张全新的图并记下序列句柄(首屏 / 切主题 / 连接后重对齐都走它)。
  function renderBars(box, bars) {
    destroyChart();
    box.innerHTML = "";
    chartSeries = null;
    chartData = bars.slice();
    if (!bars.length) {
      box.appendChild(h("div", "mk-empty mk-chart-empty", t("mkChartEmpty")));
      return;
    }
    const border = themeColor("--border", "#2b333d");
    const LWC = window.LightweightCharts;
    // 涨跌配色 = 项目既有约定(涨红跌绿, 与指标卡/历史表一致): 涨 = --error, 跌 = --accent。
    const upColor = themeColor("--error", "#f85149");
    const downColor = themeColor("--accent", "#4dd4ac");
    chart = LWC.createChart(box, {
      // autoSize: v4 内置 ResizeObserver 自适应容器宽高, 无需手写 resize 监听。
      autoSize: true,
      layout: { background: { color: "transparent" }, textColor: themeColor("--muted", "#9aa7b4") },
      grid: {
        vertLines: { color: border },
        horzLines: { color: border },
      },
      timeScale: { timeVisible: true, secondsVisible: false, borderColor: border },
    });
    const candle = chart.addCandlestickSeries({
      upColor,
      downColor,
      borderVisible: false,
      wickUpColor: upColor,
      wickDownColor: downColor,
    });
    candle.setData(chartData);

    // ---- 039: 成交量副图 (独立价格轴占底部约 18%; 与主图共享时间轴, 无需同步逻辑) ----
    const vol = chart.addHistogramSeries({
      priceScaleId: "vol",
      priceFormat: { type: "volume" },
      priceLineVisible: false,
      lastValueVisible: false,
    });
    chart.priceScale("vol").applyOptions({ scaleMargins: { top: VOL_PANE_MARGIN, bottom: 0 } });
    vol.setData(
      chartData
        .filter((d) => Number.isFinite(d.volume))
        .map((d) => ({
          time: d.time,
          value: d.volume,
          color: d.close >= d.open ? upColor : downColor,
        }))
    );

    // ---- 039: MA7 / MA25 / MA99 (简单移动平均, 收盘价口径) ----
    const ma = [];
    for (let i = 0; i < MA_WINDOWS.length; i++) {
      const w = MA_WINDOWS[i];
      const line = chart.addLineSeries({
        color: themeColor(MA_VARS[i], upColor),
        lineWidth: 1,
        priceLineVisible: false,
        lastValueVisible: false,
        crosshairMarkerVisible: false,
      });
      line.setData(maSeriesData(chartData, w));
      ma.push(line);
    }
    chartSeries = { candle: candle, vol: vol, ma: ma, up: upColor, down: downColor };
    chart.timeScale().fitContent();
  }

  function destroyChart() {
    if (chart) {
      chart.remove();
      chart = null;
    }
  }

  /// 数值展示规整: 截掉浮点尾差与多余尾零(仅用于现价均值 / 图表坐标, 盘口价仍显原始字符串)。
  function formatNum(v) {
    if (!Number.isFinite(v)) return "--";
    return String(Number(v.toFixed(8)));
  }

  // 切语言时动态文本(视野标注)立即换; 其余静态标签由公共 applyLang 的 data-zh/data-en 覆盖。
  const baseApplyLang = R.applyLang;
  R.applyLang = function (lang) {
    const ret = baseApplyLang.call(this, lang);
    try {
      if (mounted && listState.data) renderScope(listState.data);
    } catch (_) {
      // 视图尚未挂载时无 DOM 可改, 忽略。
    }
    return ret;
  };

  // 034: 切主题时 K 线图跟着换配色 —— 用缓存的 K 线就地重绘, 不重新发请求。
  window.addEventListener("ricow:theme", () => {
    try {
      if (chart && chartData.length && chart.parentElement) {
        // 用**当前图上的数据**(含实时帧)重绘, 不重新发请求。
        const box = chart.parentElement;
        renderBars(box, chartData);
      }
    } catch (_) {
      // 主题广播到达时视图可能正在切换, 图表已被销毁: 忽略, 下次进详情按新主题重取。
    }
  });

  R.views.markets = {
    /// `param` = 二级 hash 解码后的符号; 缺省 = 列表态。
    activate: (param) => {
      mount();
      const g = (gen += 1); // 新激活即作废旧代际(从详情返回列表 / 列表再进详情同理)
      R.applyLang(R.lang);
      $("mk-listview").hidden = !!param;
      $("mk-detail").hidden = !param;
      if (param) enterDetail(param, g);
      else enterList(g);
    },
    deactivate: () => {
      gen += 1; // 离开视图: 在途响应全部作废, 一个 DOM 节点都不许再改
      clearTimeout(searchTimer);
      // 040: 离开视图必须断流 —— 服务端按引用计数回收上游币安连接, 不断就会一直拉着。
      closeStream();
      destroyChart();
    },
  };
})();
