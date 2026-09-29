// Ricow Web 前端 hash 路由 (032): `#chat`(默认) / `#markets` / `#strategies` / `#runs` /
// `#settings`, 二级 `#markets/{symbol}`、`#strategies/{id}`(本阶段只解析, 业务视图后续阶段接入)。
//
// 视图模块往 `R.views.<name>` 注册 `{ activate(param), deactivate() }`(钩子均可缺省);
// 本文件只负责切 `.view.active` 与调钩子, 不含任何业务逻辑。刷新按当前 hash 恢复;
// 无匹配 hash 一律回退 `#chat`。
"use strict";

(function () {
  const R = (window.Ricow = window.Ricow || {});

  /// 视图注册表: 各视图文件(chat.js / 后续 settings.js …)往这里挂自己的钩子。
  R.views = R.views || {};

  /// 一级视图名白名单: hash 首段不在这里面(且 hash 非空)即视为无匹配, 回退 #chat。
  const VIEW_NAMES = ["chat", "markets", "strategies", "runs", "settings"];

  /// 当前路由: `{ name, param }`, 首次跳转前为 null。
  let current = null;

  /// 解析 `location.hash` → `{ name, param, invalid }`。
  /// 空 hash 当作 chat(默认页); 二级段做一次 decodeURIComponent(畸形转义不抛错, 原样给)。
  function parseHash() {
    const body = (location.hash || "").replace(/^#/, "");
    const segs = body.split("/");
    const rawName = segs[0] || "chat";
    const name = VIEW_NAMES.includes(rawName) ? rawName : "chat";
    let param = null;
    if (segs.length > 1) {
      try {
        param = decodeURIComponent(segs.slice(1).join("/"));
      } catch (_) {
        param = segs.slice(1).join("/");
      }
    }
    return { name: name, param: param, invalid: body !== "" && rawName !== name };
  }

  /// 同步左侧导航高亮: 高亮目标等于当前一级视图的那一项。
  function renderNav(name) {
    for (const a of document.querySelectorAll("#nav a")) {
      const target = (a.getAttribute("href") || "").replace(/^#/, "");
      a.classList.toggle("active", target === name);
    }
  }

  /// 只显示 `#view-<name>`, 其余 `.view` 收起(同时切 class 与 hidden 属性)。
  function showSection(name) {
    for (const section of document.querySelectorAll(".view")) {
      const on = section.id === "view-" + name;
      section.classList.toggle("active", on);
      section.hidden = !on;
    }
  }

  /// 按当前 hash 切一次视图: 收起旧视图(可选 deactivate) → 切 DOM / 导航 → 激活新视图。
  function applyRoute() {
    const route = parseHash();
    if (route.invalid) {
      // 无匹配 hash 回退 #chat: replaceState 不往历史栈里塞坏 hash, 也不会再触发 hashchange。
      history.replaceState(null, "", "#chat");
      route.name = "chat";
      route.param = null;
    }
    if (current && current.name === route.name && current.param === route.param) return;
    if (current) {
      const oldView = R.views[current.name];
      if (oldView && typeof oldView.deactivate === "function") oldView.deactivate();
    }
    current = { name: route.name, param: route.param };
    showSection(route.name);
    renderNav(route.name);
    const view = R.views[route.name];
    if (view && typeof view.activate === "function") view.activate(route.param);
  }

  window.addEventListener("hashchange", applyRoute);

  /// 编程式跳转: 写 `location.hash`(自动触发 hashchange); hash 没变就手动切一次。
  R.navigate = function (hash) {
    const target = hash.charAt(0) === "#" ? hash : "#" + hash;
    if (location.hash === target) applyRoute();
    else location.hash = target;
  };

  /// 首跳: DOMContentLoaded 后由入口(app.js)调用 —— 刷新时按 hash 恢复原视图。
  R.startRouter = applyRoute;
})();
