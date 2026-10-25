// Ricow Web 前端通用 UI 组件层 (062) —— **fmz.com 视觉语言的实现**。
//
// ── 为什么有这层 ────────────────────────────────────────────────────────────────
// 062 把视觉基准换成 fmz.com(发明者量化)后, 卡片 / 标签页 / 空态 / 面包屑 / 按钮
// 这五种形态在**每个视图**都要出现。若各视图各写一份, 同一个"卡片"在策略页是圆角 4px、
// 在密钥页是圆角 8px —— 视觉立刻就不齐, 且改一次要改 N 处。故抽到这里, 一处实现、各处调用。
//
// ── 为什么手写而不引 antd ────────────────────────────────────────────────────────
// fmz 用的是 Ant Design, 但本项目: ① CSP 是 `script-src 'self'`, 引不了 CDN;
// ② 前端是**零依赖原生 JS**(`include_str!` 嵌进二进制), 引 npm 包等于给"单二进制、零供应链"
// 的取向破例; ③ 只需要 5 种形态, 不值得背一个组件库的体积。
// 故: **照着 fmz 实测值手写实现** —— 圆角 4px、卡片描边 #F0F0F0、按钮高 32px、标签页高 50px。
//
// ── 纪律 ────────────────────────────────────────────────────────────────────────
// ① **只产 DOM 节点, 不拼 HTML 字符串** —— 用户数据拼 HTML 是红线(见 constitution),
//    所以这里没有一处 `innerHTML =`; 文本一律 `textContent`。
// ② **不引入任何浏览器端持久化** —— 前端的"状态"只准来自服务端事实, 不落浏览器存储
//    (这条门禁是按**字面量**扫全文件的, 连注释里出现存储 API 的名字都会算命中 ——
//    故此处只说"不落浏览器存储", 不写出那个 API 名)。
// ③ 本层**不含业务逻辑** —— 只管长什么样; 数据从哪来、点了做什么, 由调用方给回调。
// ④ 加载顺序: 必须在 `common.js` 之后(`R.t()` 可用)、`router.js` 之前(不注册视图,
//    但要能用 `R.t()`); 见 `assets.rs` 的顺序契约与 `index.html` 底部。
"use strict";

(function () {
  const R = (window.Ricow = window.Ricow || {});

  /// DOM 小工具: 建元素(不拼 HTML)。
  function h(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = String(text);
    return node;
  }

  /// 建 `<svg>` 图标节点(内联, 不引外部资源)。
  /// `paths` 是 SVG 子元素描述数组: `{ tag, attrs }`。
  function svgNode(paths, size) {
    const NS = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("width", String(size || 16));
    svg.setAttribute("height", String(size || 16));
    svg.setAttribute("aria-hidden", "true");
    for (const p of paths || []) {
      const el = document.createElementNS(NS, p.tag || "path");
      for (const k in p.attrs || {}) {
        el.setAttribute(k, String(p.attrs[k]));
      }
      svg.appendChild(el);
    }
    return svg;
  }

  // ── 图标集 ────────────────────────────────────────────────────────────────────
  // 自绘内联 SVG(24×24 viewBox, 1.5~1.6 线宽, currentColor) —— 对齐 fmz 侧栏那套线性图标。
  // 不引 `@ant-design/icons`: 那是 npm 包, 见文件头"为什么手写而不引 antd"。
  const ICONS = {
    dashboard: [
      { tag: "rect", attrs: { x: 3, y: 3, width: 7, height: 9, rx: 1 } },
      { tag: "rect", attrs: { x: 14, y: 3, width: 7, height: 5, rx: 1 } },
      { tag: "rect", attrs: { x: 14, y: 12, width: 7, height: 9, rx: 1 } },
      { tag: "rect", attrs: { x: 3, y: 16, width: 7, height: 5, rx: 1 } },
    ],
    markets: [{ tag: "path", attrs: { d: "M3 12h4l3 8 4-16 3 8h4" } }],
    strategies: [
      { tag: "path", attrs: { d: "M4 5h16v14H4z" } },
      { tag: "path", attrs: { d: "M4 9h16M9 9v10" } },
    ],
    runs: [
      { tag: "circle", attrs: { cx: 12, cy: 12, r: 3 } },
      { tag: "path", attrs: { d: "M12 2v4M12 18v4M2 12h4M18 12h4" } },
    ],
    keys: [
      { tag: "path", attrs: { d: "M14 7a4 4 0 11-3 7l-4 4v-3H4v-4h3z" } },
    ],
    chat: [
      { tag: "path", attrs: { d: "M21 11.5a8.4 8.4 0 01-9 8.4 8.9 8.9 0 01-4-.9L3 21l1.9-4.8A8.4 8.4 0 0121 11.5z" } },
    ],
    // 064: 告警铃铛(fmz 的概览"告警消息"格用铃铛; 我们的告警格同形)。
    alert: [
      { tag: "path", attrs: { d: "M18 8a6 6 0 10-12 0c0 7-3 9-3 9h18s-3-2-3-9" } },
      { tag: "path", attrs: { d: "M13.7 21a2 2 0 01-3.4 0" } },
    ],
    settings: [
      { tag: "circle", attrs: { cx: 12, cy: 12, r: 3 } },
      {
        tag: "path",
        attrs: {
          d: "M19.4 15a1.7 1.7 0 00.3 1.9l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.9-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1A1.7 1.7 0 007 19.4a1.7 1.7 0 00-1.9.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 001 1.5H3a2 2 0 110-4h.1A1.7 1.7 0 004.6 7a1.7 1.7 0 00-.3-1.9l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.9.3H9a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.9-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.9V9a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z",
        },
      },
    ],
    plus: [{ tag: "path", attrs: { d: "M12 5v14M5 12h14" } }],
    search: [
      { tag: "circle", attrs: { cx: 11, cy: 11, r: 7 } },
      { tag: "path", attrs: { d: "M20 20l-3.5-3.5" } },
    ],
    arrow: [{ tag: "path", attrs: { d: "M9 6l6 6-6 6" } }],
    warn: [
      { tag: "path", attrs: { d: "M12 9v4M12 17h.01" } },
      {
        tag: "path",
        attrs: { d: "M10.3 3.9L2.4 17.5A2 2 0 004.1 20.5h15.8a2 2 0 001.7-3L13.7 3.9a2 2 0 00-3.4 0z" },
      },
    ],
    empty: [
      { tag: "path", attrs: { d: "M12 28h40v22a4 4 0 01-4 4H16a4 4 0 01-4-4z" } },
      { tag: "path", attrs: { d: "M8 16h48v12H8z" } },
      { tag: "path", attrs: { d: "M22 38h20" } },
    ],
  };

  R.ui = R.ui || {};

  /// 取图标节点。`name` 不认识时返回一个空占位 svg(不留空洞、不抛错)。
  R.ui.icon = function (name, size) {
    return svgNode(ICONS[name] || [], size || 16);
  };

  /// 导航图标 → 视图名(供 router / 各视图把图标塞进 `<nav>`)。
  R.ui.NAV_ICON = {
    dashboard: "dashboard",
    markets: "markets",
    strategies: "strategies",
    runs: "runs",
    keys: "keys",
    chat: "chat",
    settings: "settings",
  };

  // ── 卡片(fmz 的 `.ant-card`: 白底 + #F0F0F0 描边 + 4px 圆角, 无阴影) ──────────
  //
  /// opts: { title?, extra?: Node[], body?: Node|string, className?, noBody? }
  /// 返回卡片根节点; 头部在有 title 或 extra 时才建(fmz 的卡片可以只有 body)。
  R.ui.card = function (opts) {
    const o = opts || {};
    const card = h("div", "ui-card" + (o.className ? " " + o.className : ""));
    if (o.title || (o.extra && o.extra.length)) {
      const head = h("div", "ui-card-head");
      if (o.title) head.appendChild(h("span", "ui-card-title", o.title));
      if (o.extra && o.extra.length) {
        const acts = h("div", "ui-card-actions");
        for (const node of o.extra) if (node) acts.appendChild(node);
        head.appendChild(acts);
      }
      card.appendChild(head);
    }
    if (!o.noBody) {
      const body = h("div", "ui-card-body");
      if (typeof o.body === "string") body.textContent = o.body;
      else if (o.body) body.appendChild(o.body);
      card.appendChild(body);
    }
    return card;
  };

  // ── 标签页(fmz 的 `.ant-tabs`: 高 50px, 选中项主色 + 下划线) ─────────────────
  //
  /// opts: { items: [{ key, label, count? }], active?, onChange?(key) }
  /// 返回标签页根节点; 点击项调 `onChange(key)` —— **不自己切**, 由调用方决定状态。
  R.ui.tabs = function (opts) {
    const o = opts || {};
    const box = h("div", "ui-tabs");
    box.setAttribute("role", "tablist");
    for (const item of o.items || []) {
      const tab = h("div", "ui-tab" + (item.key === o.active ? " on" : ""));
      tab.setAttribute("role", "tab");
      tab.setAttribute("tabindex", "0");
      tab.appendChild(h("span", "ui-tab-label", item.label));
      if (item.count !== undefined && item.count !== null) {
        tab.appendChild(h("span", "ui-tab-count", String(item.count)));
      }
      const pick = () => {
        if (typeof o.onChange === "function") o.onChange(item.key);
      };
      tab.addEventListener("click", pick);
      tab.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter" || ev.key === " ") {
          ev.preventDefault();
          pick();
        }
      });
      box.appendChild(tab);
    }
    return box;
  };

  // ── 空态(fmz 的 `.ant-empty`: 插画 + 文案 + 可选 CTA) ────────────────────────
  //
  /// opts: { text, cta?: { label, onClick }, icon? }
  R.ui.empty = function (opts) {
    const o = opts || {};
    const box = h("div", "ui-empty");
    const art = h("div", "ui-empty-art");
    art.appendChild(svgNode(o.icon === "mail" ? ICONS.empty : ICONS.empty, 56));
    box.appendChild(art);
    if (o.text) box.appendChild(h("div", "ui-empty-text", o.text));
    if (o.cta && o.cta.label) {
      const btn = R.ui.button({ kind: "default", label: o.cta.label, onClick: o.cta.onClick });
      const wrap = h("div", "ui-empty-cta");
      wrap.appendChild(btn);
      box.appendChild(wrap);
    }
    return box;
  };

  // ── 面包屑(fmz: 「控制中心 / 策略库」, 三级文字色) ──────────────────────────
  //
  /// opts: { items: [{ label, href? }] }  —— 有 href 的项渲染成可点链接。
  R.ui.breadcrumb = function (opts) {
    const o = opts || {};
    const nav = h("nav", "ui-crumbs");
    nav.setAttribute("aria-label", "面包屑 / Breadcrumb");
    const items = o.items || [];
    items.forEach((item, i) => {
      if (i > 0) nav.appendChild(h("span", "ui-crumb-sep", "/"));
      if (item.href) {
        const a = h("a", "ui-crumb-link", item.label);
        a.setAttribute("href", item.href);
        nav.appendChild(a);
      } else {
        nav.appendChild(h("span", "ui-crumb-current", item.label));
      }
    });
    return nav;
  };

  // ── 按钮(fmz 的 `.ant-btn`: 高 32px, 圆角 4px, 三档) ─────────────────────────
  //
  /// opts: { label, kind?: "primary"|"default"|"text"|"ghost"|"danger", icon?, onClick?, title? }
  /// kind 映射到 fmz 的三档: primary=实色主色 / default=描边主色 / text=无框主色字;
  /// "ghost" = 中性描边(非主色), "danger" = 危险(红)。
  R.ui.button = function (opts) {
    const o = opts || {};
    const kind = o.kind || "default";
    const btn = h("button", "ui-btn ui-btn-" + kind);
    btn.type = "button";
    if (o.label) btn.appendChild(h("span", "ui-btn-label", o.label));
    if (o.icon) {
      const ico = h("span", "ui-btn-ico");
      ico.appendChild(R.ui.icon(o.icon, 14));
      // 图标在文字**前**(fmz 的形态)。
      if (o.label) btn.insertBefore(ico, btn.firstChild);
      else btn.appendChild(ico);
    }
    if (o.title) {
      btn.setAttribute("title", o.title);
      btn.setAttribute("aria-label", o.title);
    }
    if (typeof o.onClick === "function") btn.addEventListener("click", o.onClick);
    return btn;
  };

  /// 建一个 `<span class="ui-tag">`(状态标签)。`tone`: ""(中性) / "on"(主色) / "warn"。
  R.ui.tag = function (text, tone) {
    return h("span", "ui-tag" + (tone ? " ui-tag-" + tone : ""), text);
  };

  /// 建一个统计块(fmz 仪表盘的「图标 + 大数字 + 名称」)。
  /// opts: { value, label, icon?, tone? } —— `value` 传 null/"" 时如实显示"—", 不显示 0。
  R.ui.stat = function (opts) {
    const o = opts || {};
    const box = h("div", "ui-stat");
    if (o.icon) {
      const ic = h("div", "ui-stat-icon");
      ic.appendChild(R.ui.icon(o.icon, 20));
      box.appendChild(ic);
    }
    const body = h("div", "ui-stat-body");
    const raw = o.value;
    body.appendChild(h("div", "ui-stat-value" + (o.tone ? " ui-stat-" + o.tone : ""), raw === null || raw === undefined || raw === "" ? "—" : String(raw)));
    body.appendChild(h("div", "ui-stat-label", o.label));
    box.appendChild(body);
    return box;
  };

  /// 建一个快捷入口方块(fmz 仪表盘的六宫格)。
  /// opts: { label, icon?, onClick? }
  R.ui.quick = function (opts) {
    const o = opts || {};
    const box = h("button", "ui-quick");
    box.type = "button";
    if (o.icon) box.appendChild(R.ui.icon(o.icon, 22));
    box.appendChild(h("span", "ui-quick-label", o.label));
    if (typeof o.onClick === "function") box.addEventListener("click", o.onClick);
    return box;
  };

  // ── 表格(fmz 的 `.ant-table`: 表头 #FBFBFB + 字重 600 + 12px 16px 内边距) ──────
  //
  /// 063: 策略库与实盘列表改用 fmz 的表格形态(此前是卡片行)。
  ///
  /// opts: { columns: [{ key, title, align? }], rows: [{ cells: { key: Node|string }, actions?: Node[] }] }
  /// 返回表格根节点。**单元格内容只走 textContent / 节点插入** —— 服务端数据绝不当 HTML 拼。
  R.ui.table = function (opts) {
    const o = opts || {};
    const table = h("table", "ui-table");
    const cols = o.columns || [];
    const head = h("thead");
    const headRow = h("tr");
    for (const col of cols) {
      const th = h("th");
      if (col.align) th.style.textAlign = col.align;
      th.appendChild(h("span", null, col.title));
      headRow.appendChild(th);
    }
    head.appendChild(headRow);
    table.appendChild(head);
    const body = h("tbody");
    for (const row of o.rows || []) {
      const tr = h("tr");
      for (const col of cols) {
        const td = h("td");
        if (col.align) td.style.textAlign = col.align;
        const cell = (row.cells || {})[col.key];
        if (cell === null || cell === undefined) {
          // 空单元格显示"—"(读不到不是 0)。
          td.appendChild(h("span", "ui-cell-empty", "—"));
        } else if (typeof cell === "string") {
          td.appendChild(h("span", "ui-cell-text", cell));
        } else {
          td.appendChild(cell);
        }
        tr.appendChild(td);
      }
      body.appendChild(tr);
    }
    table.appendChild(body);
    const wrap = h("div", "ui-table-wrap");
    wrap.appendChild(table);
    return wrap;
  };

  /// 建一个**行内文字链接**(fmz 表格「操作项」列的形态: 蓝色文字, 无框)。
  /// opts: { label, onClick?, tone?: "default"|"danger" }
  ///
  /// 刻意做成 `<button>` 而不是 `<a href="#">`: 死链接会让用户点了没反应,
  /// 而这类动作都是"执行一次"(复制/删除/编辑), 没有可分享的地址。
  R.ui.linkBtn = function (opts) {
    const o = opts || {};
    const btn = h("button", "ui-linkbtn" + (o.tone === "danger" ? " ui-linkbtn-danger" : ""));
    btn.type = "button";
    btn.appendChild(h("span", null, o.label));
    if (typeof o.onClick === "function") btn.addEventListener("click", o.onClick);
    return btn;
  };

  /// 给导航项注入图标位(只注入一次; 已有图标位则跳过)。
  /// `nav` 是 `<nav>` 元素; 视图名 → 图标名的映射见 `R.ui.NAV_ICON`。
  R.ui.decorateNav = function (nav) {
    if (!nav) return;
    for (const a of nav.querySelectorAll("a[href^='#']")) {
      const view = (a.getAttribute("href") || "").replace(/^#/, "");
      const iconName = R.ui.NAV_ICON[view];
      if (!iconName) continue;
      if (a.querySelector(".nav-ico")) continue;
      const slot = h("span", "nav-ico");
      slot.appendChild(R.ui.icon(iconName, 16));
      a.insertBefore(slot, a.firstChild);
    }
  };
})();
