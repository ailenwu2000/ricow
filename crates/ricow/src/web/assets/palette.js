// Ricow Web 前端「命令面板」(帮助计划 B7): Ctrl/⌘+K 打开, 一个输入框把五类条目搜出来 ——
// 跳视图 / 开帮助段 / 切语言 / 切主题 / 查术语 / 找策略。纯前端, 不新增任何后端接口。
//
// ── 为什么它值得存在 ───────────────────────────────────────────────────────────────
// 左侧有 6 个导航项, 顶栏有主题与语言, 术语散落在对话 / 回测 / 市场各页。用户**知道要什么**
// 却**不知道在哪儿**时, 挨个点一遍是最常见的浪费。面板把"去哪儿"变成"打几个字"。
//
// ── 单一真相纪律(与 help.js 同款) ─────────────────────────────────────────────────
// 面板**不新写一份文案**, 六类条目全部从已有来源现取:
//   ① 视图   ← 左侧 `<nav>` 各链接的 `data-zh` / `data-en`(导航文案的唯一来源)
//   ② 帮助段 ← `R.TEXT` 里 help.js 已注册的段标题(helpStart / helpFaq)
//   ③ 语言   ← 语言名本身就是两份文案(中文 / English), 只能自带; 动作复用 `/api/lang`
//   ④ 主题   ← 顶栏 `#theme-select` 各 `<option>` 的 `data-zh` / `data-en`(主题文案的唯一来源)
//   ⑤ 术语   ← 公共件的全局单例 `R.glossary`(源头 `/api/terms`, 前端**只此一处**取)
//   ⑥ 策略   ← `/api/strategies`(与对话面板同一接口, 懒取一次)
// 一旦这里开始手抄"有哪些视图""主题都叫什么", 就是第二份真相: 改一处漏一处。
//
// ── 按键纪律 ─────────────────────────────────────────────────────────────────────
// 全局只监听两类: 打开它自己(`Ctrl/⌘+K`)、以及**面板开着时**的方向键 / Enter / Esc。
// 面板关着时不碰方向键与 Enter —— 用户在对话输入框里打字、在回测表单里按回车, 一律不受影响。
// 面板的输入框在面板内部消化自己的按键, 故"不拦截输入框内的按键"这条自然成立。
//
// 安全: 不写浏览器存储; 服务端文本(术语解释 / 策略名)一律 `textContent`; 无内联事件。
"use strict";

(function () {
  const R = window.Ricow;

  // ---------- 文案(并入公共字典; 自带的就是"语言名"与分组名) ----------
  Object.assign(R.TEXT.zh, {
    cmdBtnTitle: "命令面板 (Ctrl/⌘ + K)",
    cmdPh: "输入命令、视图、术语或策略…",
    cmdEmpty: "没有匹配的条目。",
    cmdCount: " 条结果",
    cmdGroupView: "视图",
    cmdGroupHelp: "帮助",
    cmdGroupLang: "语言",
    cmdGroupTheme: "主题",
    cmdGroupTerm: "术语",
    cmdGroupStrategy: "策略",
    cmdLangZh: "中文",
    cmdLangEn: "English",
    cmdCurrent: "当前",
    cmdHintView: "跳转",
    cmdHintLang: "切换界面语言",
    cmdHintTheme: "切换配色",
    cmdHintTerm: "看解释",
    cmdHintStrategy: "打开策略详情",
  });
  Object.assign(R.TEXT.en, {
    cmdBtnTitle: "Command palette (Ctrl/⌘ + K)",
    cmdPh: "Type a command, view, term or strategy…",
    cmdEmpty: "No matching item.",
    cmdCount: " results",
    cmdGroupView: "Views",
    cmdGroupHelp: "Help",
    cmdGroupLang: "Language",
    cmdGroupTheme: "Theme",
    cmdGroupTerm: "Glossary",
    cmdGroupStrategy: "Strategies",
    cmdLangZh: "中文",
    cmdLangEn: "English",
    cmdCurrent: "current",
    cmdHintView: "Go to",
    cmdHintLang: "Switch UI language",
    cmdHintTheme: "Switch colour scheme",
    cmdHintTerm: "See explanation",
    cmdHintStrategy: "Open strategy details",
  });

  const t = (key) => R.t(key);
  const $ = (id) => document.getElementById(id);

  // ---------- 数据源 ----------

  /// 导航项: 直接读左侧 `<nav>` 的双语标签 —— 不在这里再写一份"有哪些视图、分别叫什么"。
  function viewItems() {
    const out = [];
    for (const a of document.querySelectorAll("#nav a")) {
      const hash = (a.getAttribute("href") || "").replace(/^#/, "");
      if (!hash) continue;
      const label = a.getAttribute("data-" + R.lang) || a.textContent.trim();
      out.push({
        group: "view",
        label: label,
        hint: t("cmdHintView"),
        keys: hash,
        run: () => R.navigate(hash),
      });
    }
    return out;
  }

  /// 帮助段: 段标题取自 `R.TEXT`(help.js 注册的那两份), 打开抽屉并滚到那一段。
  function helpItems() {
    const out = [];
    if (!R.help || !R.help.open) return out;
    for (const [sec, key] of [["start", "helpStart"], ["faq", "helpFaq"]]) {
      const label = R.TEXT[R.lang][key];
      if (!label) continue; // help.js 没加载时不编一个标题出来
      out.push({
        group: "help",
        label: label,
        keys: "help " + sec,
        run: () => R.help.open(sec),
      });
    }
    return out;
  }

  function langItems() {
    const langs = [
      { code: "zh", label: t("cmdLangZh") },
      { code: "en", label: t("cmdLangEn") },
    ];
    return langs.map((l) => ({
      group: "lang",
      label: l.label,
      // "当前"标记本身也要本地化 —— 写死成中文词会让英文界面出现 "· 中文" 这种混排。
      hint: t("cmdHintLang") + (R.lang === l.code ? " · " + t("cmdCurrent") : ""),
      keys: l.code,
      run: () => switchLang(l.code),
    }));
  }

  /// 主题: 选项文案读顶栏那个 `<select>`(它才是主题文案的唯一来源)。
  function themeItems() {
    const sel = $("theme-select");
    if (!sel) return [];
    return Array.from(sel.options).map((o) => ({
      group: "theme",
      label: o.getAttribute("data-" + R.lang) || o.textContent.trim(),
      hint: t("cmdHintTheme"),
      keys: o.value,
      run: () => switchTheme(o.value),
    }));
  }

  function termItems() {
    return R.glossary.all().map((term) => ({
      group: "term",
      label: term.key,
      hint: t("cmdHintTerm"),
      keys: (term.zh || "") + " " + (term.en || ""),
      // 术语**不关面板**: 就地给解释(见 renderDetail), 用户看完接着搜下一个。
      detail: () => R.glossary.text(term.key),
    }));
  }

  let stratCache = null; // 懒取一次; 失败不粘住(下次开面板重试)
  function strategyItems() {
    if (!stratCache) return [];
    const builtin = R.TEXT[R.lang].sgBuiltin || "";
    const mine = R.TEXT[R.lang].sgUser || "";
    return stratCache.map((s) => ({
      group: "strategy",
      label: s.name || s.id,
      hint: (s.source === "builtin" ? builtin : mine) + " · " + t("cmdHintStrategy"),
      keys: s.id + " " + (s.summary || ""),
      run: () => R.navigate("strategies/" + encodeURIComponent(s.id)),
    }));
  }

  function loadStrategies() {
    if (stratCache) return Promise.resolve();
    return R.api("/api/strategies")
      .then((rows) => {
        stratCache = rows || [];
      })
      .catch(() => {
        stratCache = null; // 读不到就下次再试, **不编一份空目录**
      });
  }

  // ---------- 动作 ----------

  async function switchLang(code) {
    if ((code === "en" ? "en" : "zh") === R.lang) return;
    try {
      const info = await R.api("/api/lang", { method: "POST", body: { lang: code } });
      R.applyLang(info.lang);
    } catch (err) {
      R.toast(t("failed") + (err && err.message), "err");
    }
  }

  async function switchTheme(theme) {
    if (theme === R.theme) return;
    try {
      const info = await R.api("/api/theme", { method: "POST", body: { theme: theme } });
      R.applyTheme(info.theme);
    } catch (err) {
      R.toast(t("failed") + (err && err.message), "err");
    }
  }

  // ---------- 面板 ----------

  const GROUP_ORDER = ["view", "help", "strategy", "term", "lang", "theme"];
  const GROUP_KEY = {
    view: "cmdGroupView",
    help: "cmdGroupHelp",
    strategy: "cmdGroupStrategy",
    term: "cmdGroupTerm",
    lang: "cmdGroupLang",
    theme: "cmdGroupTheme",
  };

  let rows = []; // 平铺后的可选项(方向键走的就是这个序)
  let sel = 0; // 当前高亮下标

  const isOpen = () => {
    const box = $("palette");
    return !!box && !box.hidden;
  };

  /// 全部条目(按语言现取)。术语与策略可能还没到 —— 到了会重画(见 open)。
  function allItems() {
    return [
      ...viewItems(),
      ...helpItems(),
      ...strategyItems(),
      ...termItems(),
      ...langItems(),
      ...themeItems(),
    ];
  }

  /// 子串匹配: 词条名 / 别名(keys) / 提示文本都参与; 大小写不敏感。
  function matches(item, q) {
    if (!q) return true;
    const hay = (item.label + " " + (item.keys || "") + " " + (item.hint || "")).toLowerCase();
    return hay.includes(q);
  }

  function paint() {
    const list = $("palette-list");
    const input = $("palette-input");
    const empty = $("palette-empty");
    const count = $("palette-count");
    list.textContent = "";
    rows = [];
    sel = 0;

    const q = input.value.trim().toLowerCase();
    const items = allItems().filter((it) => matches(it, q));

    if (!items.length) {
      empty.hidden = false;
      empty.textContent = t("cmdEmpty");
      count.textContent = "0" + t("cmdCount");
      return;
    }
    empty.hidden = true;

    for (const g of GROUP_ORDER) {
      const group = items.filter((it) => it.group === g);
      if (!group.length) continue;
      const head = document.createElement("div");
      head.className = "palette-group";
      head.textContent = t(GROUP_KEY[g]);
      list.appendChild(head);
      for (const item of group) {
        const idx = rows.length;
        const row = document.createElement("div");
        row.className = "palette-item";
        row.id = "palette-item-" + idx;
        row.setAttribute("role", "option");
        row.setAttribute("aria-selected", "false");
        const label = document.createElement("span");
        label.className = "palette-item-label";
        label.textContent = item.label;
        row.appendChild(label);
        if (item.hint) {
          const hint = document.createElement("span");
          hint.className = "palette-item-hint muted";
          hint.textContent = item.hint;
          row.appendChild(hint);
        }
        row.addEventListener("mousemove", () => highlight(idx));
        row.addEventListener("click", () => {
          highlight(idx);
          commit();
        });
        list.appendChild(row);
        rows.push(item);
      }
    }
    count.textContent = rows.length + t("cmdCount");
    highlight(0);
  }

  function highlight(idx) {
    if (!rows.length) return;
    sel = Math.max(0, Math.min(rows.length - 1, idx));
    for (const node of document.querySelectorAll("#palette-list .palette-item")) {
      const on = node.id === "palette-item-" + sel;
      node.classList.toggle("on", on);
      node.setAttribute("aria-selected", on ? "true" : "false");
      if (on) node.scrollIntoView({ block: "nearest" });
    }
    const input = $("palette-input");
    if (input) input.setAttribute("aria-activedescendant", "palette-item-" + sel);
  }

  /// 术语的解释就地显示(不关面板); 其余条目执行动作并关面板。
  function commit() {
    const item = rows[sel];
    if (!item) return;
    if (item.detail) {
      renderDetail(item);
      return;
    }
    close();
    try {
      item.run();
    } catch (err) {
      R.toast(t("failed") + (err && err.message), "err");
    }
  }

  function renderDetail(item) {
    const box = $("palette-detail");
    if (!box) return;
    box.textContent = "";
    const name = document.createElement("div");
    name.className = "palette-detail-name";
    name.textContent = item.label;
    const body = document.createElement("div");
    body.className = "palette-detail-body";
    body.textContent = item.detail(); // 服务端文本只走 textContent
    box.appendChild(name);
    box.appendChild(body);
    box.hidden = false;
  }

  function open() {
    const box = $("palette");
    if (!box || isOpen()) return;
    if (R.help) R.help.close(); // 两层浮层不叠着; 抽屉先收
    box.hidden = false;
    document.body.classList.add("palette-open");
    const btn = $("palette-btn");
    if (btn) btn.setAttribute("aria-expanded", "true");
    const input = $("palette-input");
    input.value = "";
    input.placeholder = t("cmdPh");
    const detail = $("palette-detail");
    if (detail) {
      detail.hidden = true;
      detail.textContent = "";
    }
    paint();
    input.focus();
    // 术语 / 策略晚到就补一次(仍按用户此刻的输入过滤, 不覆盖他打的字)。
    Promise.all([R.glossary.load().catch(() => {}), loadStrategies()]).then(() => {
      if (isOpen()) paint();
    });
  }

  function close() {
    const box = $("palette");
    if (!box || !isOpen()) return;
    box.hidden = true;
    document.body.classList.remove("palette-open");
    const btn = $("palette-btn");
    if (btn) btn.setAttribute("aria-expanded", "false");
    const input = $("palette-input");
    if (input) input.setAttribute("aria-activedescendant", "");
  }

  const toggle = () => (isOpen() ? close() : open());

  /// 按钮的悬浮说明按语言走(它不挂 `data-zh`, 免得图标标签被语言切换整个换掉)。
  function syncBtnTitle() {
    const btn = $("palette-btn");
    if (btn) btn.title = t("cmdBtnTitle");
  }

  function wire() {
    const btn = $("palette-btn");
    const overlay = $("palette-overlay");
    const input = $("palette-input");
    if (!input) return; // 骨架不在(理论上不会): 静默不启用, 不影响其余功能
    if (btn) {
      btn.addEventListener("click", toggle);
      // 按钮 label 固定写 "⌘K"(不挂 data-zh) —— 图标式标签不该被语言切换冲掉。
      syncBtnTitle();
    }
    if (overlay) overlay.addEventListener("click", close);

    input.addEventListener("input", paint);
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "ArrowDown") {
        ev.preventDefault();
        highlight(sel + 1);
      } else if (ev.key === "ArrowUp") {
        ev.preventDefault();
        highlight(sel - 1);
      } else if (ev.key === "Home") {
        ev.preventDefault();
        highlight(0);
      } else if (ev.key === "End") {
        ev.preventDefault();
        highlight(rows.length - 1);
      } else if (ev.key === "Enter") {
        ev.preventDefault();
        commit();
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        close();
      }
    });

    // 全局: 只认"打开它自己"这一个组合键 —— 其它按键一概不碰(见文件头按键纪律)。
    document.addEventListener("keydown", (ev) => {
      if ((ev.ctrlKey || ev.metaKey) && !ev.altKey && (ev.key === "k" || ev.key === "K")) {
        ev.preventDefault();
        toggle();
      } else if (ev.key === "Escape" && isOpen()) {
        close();
      }
    });

    // 切视图收起(它是"盖在当前页上的一层"); 与帮助抽屉同一处理。
    window.addEventListener("hashchange", () => {
      if (isOpen()) close();
    });
  }

  R.palette = { open: open, close: close, toggle: toggle };

  // 切语言时若面板正开着, 就地重画(分组名 / 词条名都要跟着换)—— 与 help.js 同一手法。
  const baseApplyLang = R.applyLang;
  R.applyLang = function (lang) {
    const ret = baseApplyLang.call(this, lang);
    try {
      syncBtnTitle();
      if (isOpen()) paint();
    } catch (_) {
      // 骨架不在时忽略, 不影响语言切换本身。
    }
    return ret;
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", wire);
  } else {
    wire();
  }
})();
