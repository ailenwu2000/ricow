// Ricow Web 前端 hash 路由 (032): `#chat` / `#markets` / `#strategies` / `#runs` /
// `#keys`(033) / `#settings`, 二级 `#markets/{symbol}`、`#strategies/{id}`(本阶段只解析, 业务视图后续阶段接入)。
//
// 062: 新增 `#dashboard`(概览) —— 视觉基准换成 fmz.com 后, fmz 的"控制中心"是默认页,
// ricow 对应的是"概览"(策略数/实例数/密钥数/快捷入口)。它排在 `VIEW_NAMES` **首位**
// (与 fmz 侧栏"控制中心"置顶一致), 但**首屏默认仍是 `#chat`** —— 见下面 `DEFAULT_VIEW`。
//
// 视图模块往 `R.views.<name>` 注册 `{ activate(param), deactivate() }`(钩子均可缺省);
// 本文件只负责切 `.view.active` 与调钩子, 不含任何业务逻辑。刷新按当前 hash 恢复;
// 无匹配 hash 一律回退默认视图。
"use strict";

(function () {
  const R = (window.Ricow = window.Ricow || {});

  /// 视图注册表: 各视图文件(chat.js / 后续 settings.js …)往这里挂自己的钩子。
  R.views = R.views || {};

  /// 一级视图名白名单: hash 首段不在这里面(且 hash 非空)即视为无匹配, 回退默认视图。
  /// **首项 = 默认视图**(062: 概览置顶, 与 fmz 侧栏一致)。
  const VIEW_NAMES = ["dashboard", "chat", "markets", "strategies", "runs", "keys", "settings"];

  /// 首屏默认视图(062: 仍是 `#chat` —— 对话是 ricow 的主入口, 概览是"看板"不是"工作台";
  /// fmz 把控制中心当默认页是因为它本身就是交易台, ricow 的交易动作仍然走对话确认流程)。
  /// 非法 hash 也回退到这里。
  const DEFAULT_VIEW = "chat";

  /// 当前路由: `{ name, param }`, 首次跳转前为 null。
  let current = null;

  /// 当前一级视图名(**唯一真相**: 只由本文件在 `applyRoute` 里写)。
  ///
  /// 051 复核 B12: 早先 `help.js` 另有一份 `syncCurrentView()` 直接从 `location.hash` 取首段,
  /// 与这里形成两份真相 —— 首屏落在非法 hash(`#nope`)时, help 读到 `"nope"`, 随后 `applyRoute`
  /// 用 `history.replaceState("#chat")` 纠正(**不触发 hashchange**), 而 help 那份再也不会更新,
  /// 帮助抽屉的「当前视图」高亮就错位。现在写入口唯一, 读方(help.js)只读不写。
  R.currentView = null;

  /// 解析 `location.hash` → `{ name, param, invalid }`。
  /// 空 hash 当作默认视图; 二级段做一次 decodeURIComponent(畸形转义不抛错, 原样给)。
  function parseHash() {
    const body = (location.hash || "").replace(/^#/, "");
    const segs = body.split("/");
    const rawName = segs[0] || DEFAULT_VIEW;
    const name = VIEW_NAMES.includes(rawName) ? rawName : DEFAULT_VIEW;
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

  /// 导出一份(可传 hash 覆盖 `location.hash`)供 **纯逻辑单测** 用 —— `scripts/web_frontend_check.mjs`
  /// 在 node 里只喂一个 `{hash}` 桩就能把路由解析的表/边界一次跑完, 不必起浏览器。
  R.parseHash = function (hash) {
    if (hash === undefined) return parseHash();
    const saved = location.hash;
    location.hash = hash;
    const out = parseHash();
    location.hash = saved;
    return out;
  };

  /// 同步左侧导航高亮: 高亮目标等于当前一级视图的那一项。
  function renderNav(name) {
    for (const a of document.querySelectorAll("#nav a")) {
      const target = (a.getAttribute("href") || "").replace(/^#/, "");
      a.classList.toggle("active", target === name);
    }
  }

  /// 059 UX-01 / UX-02: 顶栏视图名 + 浏览器标签标题。
  ///
  /// 视图名**不在这里另立一份文案** —— 直接读左侧 `<nav>` 里那一项的文本: 它与导航是同一份
  /// 数据(`data-zh`/`data-en` 由 `R.applyLang` 就地改), 于是加视图 / 改文案都只有一处要动,
  /// 也不会出现"导航写着市场、顶栏写着 Markets"的分叉。
  function viewLabel(name) {
    const a = document.querySelector('#nav a[href="#' + name + '"]');
    return (a && a.textContent.trim()) || name;
  }

  /// 写顶栏视图名与 `document.title`。切语言后要再调一次(见 common.js 的 `R.applyLang`)。
  function renderTitle(name) {
    const label = viewLabel(name);
    const vt = document.getElementById("view-title");
    if (vt) vt.textContent = label;
    // UX-02: 多标签页 / 浏览器历史里能分辨是哪个视图。
    document.title = label + " · Ricow";
  }

  /// 写面包屑(fmz 的 `.ant-breadcrumb`): 格式「概览 / 当前视图」。
  ///
  /// 062: **面包屑由 router 统一写**, 不各视图自己画 ——
  /// fmz 的面包屑是**全局的**(每个页面都有, 且第一级恒为"控制中心"), 而 ricow 的
  /// 视图各自画的话, ① 各视图要各写一遍, ② 格式必然不一致, ③ 切语言时还要各自重刷。
  /// 放在 router 里, 它就与顶栏标题一样, 切视图/切语言都会自动跟上。
  ///
  /// 第一级取**导航首项**(概览)—— 与 fmz 的"控制中心"位置对应;
  /// 第二级取当前视图名 —— 与顶栏 `#view-title` 同源(`viewLabel`)。
  function renderCrumbs(name) {
    const box = document.getElementById("crumbs");
    if (!box) return;
    box.innerHTML = "";
    const root = document.querySelector('#nav a[href="#' + (VIEW_NAMES[0] || "dashboard") + '"]');
    const rootLabel = (root && root.textContent.trim()) || "概览";
    const sep = document.createElement("span");
    sep.className = "ui-crumb-sep";
    sep.textContent = "/";
    const cur = document.createElement("span");
    cur.className = "ui-crumb-current";
    cur.textContent = viewLabel(name);
    box.appendChild(sep);
    box.appendChild(cur);
    // 第一级只在**不在概览页**时才显示(在概览页时"概览 / 概览"是废话)。
    if (name !== VIEW_NAMES[0]) {
      const first = document.createElement("span");
      first.className = "ui-crumb-link";
      first.textContent = rootLabel;
      box.insertBefore(first, sep);
    }
  }

  /// 供外部(语言切换)重刷标题: 当前视图名由本文件持有。
  R.refreshTitle = function () {
    renderTitle(R.currentView || "chat");
    // 062: 面包屑也带视图名, 切语言时要跟着重刷。
    renderCrumbs(R.currentView || "chat");
  };

  /// 只显示 `#view-<name>`, 其余 `.view` 收起(同时切 class 与 hidden 属性)。
  function showSection(name) {
    for (const section of document.querySelectorAll(".view")) {
      const on = section.id === "view-" + name;
      section.classList.toggle("active", on);
      section.hidden = !on;
    }
  }

  /// 059 UX-16: 视图可以注册一个"能不能离开"的钩子, 由**本文件在切走之前**调用。
  ///
  /// 为什么必须由 router 来问: 同一个 window 上的多个 `hashchange` 监听按**注册顺序**跑,
  /// 而 router.js 排在所有视图脚本之前(见 assets.rs 的加载顺序契约), 它的监听必然先跑 ——
  /// 视图自己再监听 hashchange 时, router 早已把旧视图 `deactivate` 了(顺手还 `closeAll()`
  /// 了弹窗), 守卫就成了永远不触发的死代码(059 第一版就栽在这: 静态门四道全绿, 只有 CDP
  /// 真点一次才现形)。顺序只能由最早注册的这一方保证。
  ///
  /// 返回 `false` = "先别切"(注册方负责把地址栏拨回去); 其余 = 放行。
  let leaveGuard = null;
  R.setLeaveGuard = function (fn) {
    leaveGuard = fn;
  };

  /// 按当前 hash 切一次视图: 收起旧视图(可选 deactivate) → 切 DOM / 导航 → 激活新视图。
  function applyRoute() {
    const route = parseHash();
    if (route.invalid) {
      // 无匹配 hash 回退默认视图: replaceState 不往历史栈里塞坏 hash, 也不会再触发 hashchange。
      history.replaceState(null, "", "#" + DEFAULT_VIEW);
      route.name = DEFAULT_VIEW;
      route.param = null;
    }
    // 051 复核 B12: 在校正之后写 `R.currentView`(唯一真相), 且写在早退判断**之前** ——
    // 否则非法 hash 回退这条路径上, 读方会一直拿着校正前的旧值。
    R.currentView = route.name;
    if (current && current.name === route.name && current.param === route.param) return;
    // 059 UX-16: 切走之前先问一句(为什么必须在这里问, 见上方 `R.setLeaveGuard` 的说明)。
    if (leaveGuard && leaveGuard(route) === false) {
      // 没切成就把唯一真相拨回旧视图 —— 否则 `R.currentView` 会指着那个"没去成"的视图。
      R.currentView = current ? current.name : route.name;
      return;
    }
    if (current) {
      const oldView = R.views[current.name];
      if (oldView && typeof oldView.deactivate === "function") oldView.deactivate();
    }
    current = { name: route.name, param: route.param };
    showSection(route.name);
    renderNav(route.name);
    // 059 UX-01 / UX-03: 视图名进顶栏与标签标题; 会话名与会话列表只在对话视图出现
    // (显隐由 `#app` 上的 `data-view` 交给 CSS, 见 style.css —— JS 不逐个改 style)。
    renderTitle(route.name);
    renderCrumbs(route.name);
    const app = document.getElementById("app");
    if (app) app.dataset.view = route.name;
    const view = R.views[route.name];
    if (view && typeof view.activate === "function") view.activate(route.param);
  }

  window.addEventListener("hashchange", applyRoute);

  /// 编程式跳转: 写 `location.hash`(自动触发 hashchange); hash 没变就手动切一次。
  ///
  /// 062: **写完 hash 还要自己切一次** —— 不能只靠 hashchange。
  /// 原因: `location.hash = ...` 在**首屏导航尚未提交**时(页面刚 load 完、URL 还没有 fragment),
  /// Chromium 会把这次 fragment 变更**并入初始 URL**, **不触发 hashchange** —— 实测
  /// `hashchange` 监听器一次都没被叫到, 而 `location.hash` 已经是目标值。
  /// 后果: 用户打开页面后**第一次**用编程式跳转(命令面板 / 空态 CTA / 概览快捷入口)
  /// 会"点了没反应"。第二次起 hash 真的变了, hashchange 正常触发, 于是看起来"第一次坏了"。
  ///
  /// 自己切一次是安全的: `applyRoute` 里有 `current.name === route.name && param 相同` 的早退,
  /// hashchange 若**也**触发了, 第二次调用直接早退, 不会重复激活视图。
  R.navigate = function (hash) {
    const target = hash.charAt(0) === "#" ? hash : "#" + hash;
    if (location.hash === target) {
      applyRoute();
      return;
    }
    location.hash = target;
    // 不等 hashchange —— 它可能永远不来(见上面的说明)。同步切一次。
    applyRoute();
  };

  /// 首跳: DOMContentLoaded 后由入口(app.js)调用 —— 刷新时按 hash 恢复原视图。
  R.startRouter = applyRoute;
})();
