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
  });

  // K 线周期按钮(默认 1h; 不含 1m/5m —— 视图契约只要 15m/1h/4h/1d)。
  const INTERVALS = ["15m", "1h", "4h", "1d"];
  const DEFAULT_INTERVAL = "1h";
  const KLINE_LIMIT = 200;
  const SEARCH_DEBOUNCE_MS = 250;

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
      ">" + R.TEXT.zh.mkBack + '</a><span id="mk-symbol" class="mk-symbol"></span></div>' +
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
    loadPrice(g, symbol, market);
    loadBook(g, symbol, market);
    loadChart(g, symbol, market, DEFAULT_INTERVAL);
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
        loadChart(g, ctx.symbol, ctx.market, iv);
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

  function renderChart(box, rows) {
    box.innerHTML = "";
    if (!rows.length) {
      box.appendChild(h("div", "mk-empty mk-chart-empty", t("mkChartEmpty")));
      return;
    }
    const LWC = window.LightweightCharts;
    chart = LWC.createChart(box, {
      // autoSize: v4 内置 ResizeObserver 自适应容器宽高, 无需手写 resize 监听。
      autoSize: true,
      layout: { background: { color: "transparent" }, textColor: "#9aa7b4" },
      grid: {
        vertLines: { color: "#2b333d" },
        horzLines: { color: "#2b333d" },
      },
      timeScale: { timeVisible: true, secondsVisible: false, borderColor: "#2b333d" },
    });
    const series = chart.addCandlestickSeries({
      upColor: "#4dd4ac",
      downColor: "#f85149",
      borderVisible: false,
      wickUpColor: "#4dd4ac",
      wickDownColor: "#f85149",
    });
    const data = rows
      .map((k) => ({
        time: Math.floor(new Date(k.open_time).getTime() / 1000),
        open: Number(k.open),
        high: Number(k.high),
        low: Number(k.low),
        close: Number(k.close),
      }))
      .filter((d) => Number.isFinite(d.time) && Number.isFinite(d.close))
      .sort((a, b) => a.time - b.time);
    series.setData(data);
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
      destroyChart();
    },
  };
})();
