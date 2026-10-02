// Ricow Web 前端「密钥」视图 (033): 多套 AI / 币安密钥的别名管理 —— 新增 / 改 / 选用 / 删除。
//
// 注册为 `R.views.keys`; 全部读写经 R.api 走 `/api/keys*`。
// 服务端模型(见 specs/changes/033-key-manager/plan.md D2 / D3):
// - 左侧列表 = 密钥环里的条目(`ricow.toml` 的 `[[ai_key]]` / `[[exchange_key]]`);
// - 「选用」= 把该套写进**生效段** `[ai]` / `[exchange]`, 之后对话与策略运行就用它;
// - 页面拿到的密钥永远是"已配置 + 尾号", 全文只进不出。
//
// 安全约束(与 032 FR-007 / FR-008 同一条契约):
// - GET 只给 configured + 尾号 hint, 密钥全文从不在前端出现;
// - 密钥输入框永远以空值挂载, 留空提交 = 不修改该项(想移除请用「删除条目」或「清除当前生效」);
// - 不写任何浏览器持久存储, 密钥只随 POST 体进本机 ricow.toml。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  // ---------- 双语文案(动态渲染部分; 静态标签同样走这套) ----------
  Object.assign(R.TEXT.zh, {
    keysTitle: "密钥",
    keysLead:
      "密钥只保存在本机配置文件 ricow.toml(权限仅当前用户可读)。给每套密钥起个别名, 就能同时保存多套、" +
      "随时切换;页面不回显已保存的全文, 密钥框留空保存表示不修改。",
    keysGroupAi: "AI 通道",
    keysGroupExchange: "币安凭据",
    keysDescAi:
      "一套 = 别名 + 服务商 + 模型 + 接口地址 + 密钥。点「选用这套」即成为当前生效的 AI 通道;自建/中转端点请填接口地址。",
    keysDescExchange:
      "一套 = 别名 + 环境 + API Key + API Secret。选用后, 该环境的凭据立即生效(实盘主网用于 live / Dry Run, 测试网用于 demo)。",
    keysAdd: "+ 新增",
    keysEmptyAi: "还没有保存过 AI 密钥。点「+ 新增」加一套, 起个别名便于日后切换。",
    keysEmptyExchange: "还没有保存过币安凭据。点「+ 新增」加一套(建议实盘只开交易权限、关闭提现)。",
    keysPick: "从左侧选一套查看/编辑, 或点「+ 新增」。",
    keysNewAi: "新增 AI 密钥",
    keysNewExchange: "新增币安凭据",
    keysEditAi: "编辑 AI 密钥",
    keysEditExchange: "编辑币安凭据",
    keysLblAlias: "别名",
    keysPhAlias: "如「工作号 DeepSeek」「备用 Kimi」",
    keysLblProvider: "服务商 (provider)",
    keysLblEnv: "环境",
    keysEnvLive: "实盘主网 (live)",
    keysEnvDemo: "测试网 demo",
    keysCustom: "自定义…",
    keysPhCustom: "自定义服务商名(需同时填接口地址)",
    keysPhModel: "留空则用该服务商的推荐模型",
    keysPhBaseUrl: "自建/中转端点才需要, 如 https://api.example.com/v1",
    keysPhKey: "留空 = 不修改",
    keysPhSecret: "留空 = 不修改",
    keysBtnUse: "选用这套",
    keysBtnSave: "保存",
    keysBtnCancel: "取消",
    keysBtnDelete: "删除",
    keysBadgeUsing: "使用中",
    keysItemNoKey: "无密钥",
    keysCurrent: "当前生效: ",
    keysCurrentNone: "未配置",
    keysFromEntry: "来自「",
    keysFromEntryEnd: "」",
    keysClear: "清除当前生效",
    keysClearLive: "清除主网凭据",
    keysClearDemo: "清除测试网凭据",
    keysTailPre: "尾号 ",
    keysSaving: "保存中…",
    keysSaved: "已保存",
    keysSaveFailed: "保存失败: ",
    keysLoadFailed: "加载密钥列表失败: ",
    keysUsing: "已切换为当前生效",
    keysUseFailed: "选用失败: ",
    keysDeleted: "已删除",
    keysDeleteFailed: "删除失败: ",
    keysCleared: "已清除当前生效凭据",
    keysClearFailed: "清除失败: ",
    keysConfirmDelete:
      "删除后不可恢复。若这套正在使用中, 当前生效的凭据会一并清空(避免留下「已删除却仍在生效」的幽灵凭据)。确定删除吗?",
    keysConfirmClear: "将清空当前生效的凭据, 密钥列表里的条目不受影响。确定吗?",
    keysNeedAlias: "请先填写别名",
    keysNeedProvider: "请先选择服务商",
    keysNeedPair: "币安凭据需要同时填写 API Key 与 API Secret",
    keysConfigured: "已配置 · ",
    keysNotSet: "未配置",
  });
  Object.assign(R.TEXT.en, {
    keysTitle: "Keys",
    keysLead:
      "Keys live only in your local ricow.toml (readable by your user alone). Give each set an alias to keep several " +
      "side by side and switch between them; full values are never shown back, and an empty key field on save means " +
      "\"leave unchanged\".",
    keysGroupAi: "AI channel",
    keysGroupExchange: "Binance credentials",
    keysDescAi:
      "A set = alias + provider + model + base URL + key. Click \"Use this set\" to make it the active AI channel; fill the base URL for self-hosted/proxy endpoints.",
    keysDescExchange:
      "A set = alias + environment + API Key + API Secret. Once used, that environment's credentials take effect immediately (mainnet for live / Dry Run, testnet for demo).",
    keysAdd: "+ Add",
    keysEmptyAi: "No AI keys saved yet. Click \"+ Add\" and give it an alias so you can switch later.",
    keysEmptyExchange: "No Binance credentials saved yet. Click \"+ Add\" (prefer a trading-only key with withdrawals off).",
    keysPick: "Pick a set on the left to view or edit, or click \"+ Add\".",
    keysNewAi: "New AI key",
    keysNewExchange: "New Binance credentials",
    keysEditAi: "Edit AI key",
    keysEditExchange: "Edit Binance credentials",
    keysLblAlias: "Alias",
    keysPhAlias: "e.g. Work DeepSeek, Backup Kimi",
    keysLblProvider: "Provider",
    keysLblEnv: "Environment",
    keysEnvLive: "Mainnet (live)",
    keysEnvDemo: "Testnet demo",
    keysCustom: "Custom…",
    keysPhCustom: "Custom provider name (base URL required too)",
    keysPhModel: "Empty = the provider's recommended model",
    keysPhBaseUrl: "Only for custom/proxy endpoints, e.g. https://api.example.com/v1",
    keysPhKey: "Empty = leave unchanged",
    keysPhSecret: "Empty = leave unchanged",
    keysBtnUse: "Use this set",
    keysBtnSave: "Save",
    keysBtnCancel: "Cancel",
    keysBtnDelete: "Delete",
    keysBadgeUsing: "In use",
    keysItemNoKey: "no key",
    keysCurrent: "Currently in use: ",
    keysCurrentNone: "not configured",
    keysFromEntry: "from \"",
    keysFromEntryEnd: "\"",
    keysClear: "Clear active credentials",
    keysClearLive: "Clear mainnet credentials",
    keysClearDemo: "Clear testnet credentials",
    keysTailPre: "ends with ",
    keysSaving: "Saving…",
    keysSaved: "Saved",
    keysSaveFailed: "Save failed: ",
    keysLoadFailed: "Failed to load keys: ",
    keysUsing: "Switched to this set",
    keysUseFailed: "Switch failed: ",
    keysDeleted: "Deleted",
    keysDeleteFailed: "Delete failed: ",
    keysCleared: "Active credentials cleared",
    keysClearFailed: "Clear failed: ",
    keysConfirmDelete:
      "This cannot be undone. If the set is currently in use, the active credentials are cleared too (so nothing keeps " +
      "running with a deleted key). Delete it?",
    keysConfirmClear: "This clears the active credentials; saved sets are untouched. Continue?",
    keysNeedAlias: "Give it an alias first",
    keysNeedProvider: "Pick a provider first",
    keysNeedPair: "Binance credentials need both the API Key and the API Secret",
    keysConfigured: "Configured · ",
    keysNotSet: "Not configured",
  });

  const KINDS = ["ai", "exchange"];
  let mounted = false;
  let data = null; // 最近一次 GET /api/keys
  let sel = { ai: null, exchange: null }; // 选中的别名; null = 未选
  let draft = { ai: false, exchange: false }; // 是否处于"新增"草稿态
  let busy = false;
  let msgSeq = 0;

  const $ = (id) => document.getElementById(id);

  /// HTML 转义: 别名 / 服务商 / 接口地址都由用户提供, 直接拼进 innerHTML 必须转义。
  function esc(s) {
    return String(s === null || s === undefined ? "" : s).replace(/[&<>"']/g, (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]),
    );
  }

  /// label + 控件 的外壳(沿用设置页的 .sf 版式)。
  function field(label, inner) {
    return '<label class="sf"><span>' + esc(label) + "</span>" + inner + "</label>";
  }

  /// 脱敏占位符: "已配置 · 尾号 xxxx" / "未配置"。
  function tailOf(secret) {
    if (!secret || !secret.configured) return t("keysNotSet");
    const hint = secret.hint || "";
    // hint 形如 "***a1b2"; 过短密钥服务端固定给 "****"(不泄露长度), 原样拼上。
    return t("keysConfigured") + (hint.length > 4 ? hint.slice(3) : hint);
  }

  function entryOf(kind) {
    if (!data || !sel[kind]) return null;
    return data[kind].entries.filter((e) => e.alias === sel[kind])[0] || null;
  }

  function presetOf(id) {
    if (!data) return null;
    return data.presets.filter((p) => p.id === id)[0] || null;
  }

  // ---------- 列表 ----------

  function aiItem(e, i) {
    const cls =
      "keys-item" +
      (sel.ai === e.alias ? " selected" : "") +
      (e.active ? " using" : "");
    const sub = esc(e.provider) + (e.model ? " · " + esc(e.model) : "");
    return (
      '<li class="' + cls + '" data-act="select" data-kind="ai" data-i="' + i + '">' +
      '<div class="keys-item-main"><span class="keys-item-alias">' + esc(e.alias) + "</span>" +
      '<span class="keys-item-sub">' + sub + "</span></div>" +
      '<div class="keys-item-side"><span class="keys-tail">' +
      esc(e.api_key.configured ? t("keysTailPre") + shortTail(e.api_key) : t("keysItemNoKey")) +
      "</span>" + (e.active ? '<span class="keys-badge">' + esc(t("keysBadgeUsing")) + "</span>" : "") +
      "</div></li>"
    );
  }

  function exchangeItem(e, i) {
    const cls =
      "keys-item" +
      (sel.exchange === e.alias ? " selected" : "") +
      (e.active ? " using" : "");
    const sub =
      (e.env === "demo" ? esc(t("keysEnvDemo")) : esc(t("keysEnvLive"))) +
      " · " + esc(t("keysTailPre") + shortTail(e.key));
    return (
      '<li class="' + cls + '" data-act="select" data-kind="exchange" data-i="' + i + '">' +
      '<div class="keys-item-main"><span class="keys-item-alias">' + esc(e.alias) + "</span>" +
      '<span class="keys-item-sub">' + sub + "</span></div>" +
      '<div class="keys-item-side">' +
      (e.active ? '<span class="keys-badge">' + esc(t("keysBadgeUsing")) + "</span>" : "") +
      "</div></li>"
    );
  }

  function shortTail(secret) {
    const hint = (secret && secret.hint) || "";
    return hint.length > 4 ? hint.slice(3) : hint;
  }

  // ---------- 详情表单 ----------

  function providerSelect(entry) {
    const ids = (data.presets || []).map((p) => p.id);
    const cur = entry ? entry.provider : (ids[0] || "");
    const isCustom = cur !== "" && ids.indexOf(cur) === -1;
    const opts = (data.presets || [])
      .map(
        (p) =>
          '<option value="' + esc(p.id) + '"' + (!isCustom && p.id === cur ? " selected" : "") + ">" +
          esc(p.id) + " · " + esc(p.label) + "</option>",
      )
      .join("");
    return (
      '<label class="sf"><span>' + esc(t("keysLblProvider")) + "</span>" +
      '<select id="k-ai-provider" class="sf-input">' + opts +
      '<option value="__custom__"' + (isCustom ? " selected" : "") + ">" + esc(t("keysCustom")) +
      "</option></select>" +
      '<input id="k-ai-provider-custom" class="sf-input" type="text" autocomplete="off" spellcheck="false" ' +
      'placeholder="' + esc(t("keysPhCustom")) + '" value="' + esc(isCustom ? cur : "") + '"' +
      (isCustom ? "" : " hidden") + "></label>"
    );
  }

  function aiForm(entry) {
    const isNew = !entry;
    const preset = presetOf(entry ? entry.provider : null) || {};
    return (
      '<form id="k-form-ai" class="keys-form" autocomplete="off">' +
      '<div class="keys-form-title">' + esc(isNew ? t("keysNewAi") : t("keysEditAi")) + "</div>" +
      field(t("keysLblAlias"), '<input id="k-ai-alias" class="sf-input" type="text" autocomplete="off" ' +
        'placeholder="' + esc(t("keysPhAlias")) + '" value="' + esc(entry ? entry.alias : "") + '">') +
      providerSelect(entry) +
      field(t("keysLblModel"), '<input id="k-ai-model" class="sf-input" type="text" autocomplete="off" ' +
        'placeholder="' + esc(preset.model || t("keysPhModel")) + '" value="' + esc(entry ? entry.model : "") + '">') +
      field(t("keysLblBaseUrl"), '<input id="k-ai-baseurl" class="sf-input" type="text" autocomplete="off" ' +
        'placeholder="' + esc(preset.base_url || t("keysPhBaseUrl")) + '" value="' + esc(entry ? entry.base_url : "") + '">') +
      field("AI API Key", '<input id="k-ai-key" class="sf-input" type="password" autocomplete="new-password" ' +
        'spellcheck="false" placeholder="' + esc(entry ? t("keysPhKey") : "") + '">') +
      hintLine("AI API Key", entry ? tailOf(entry.api_key) : "") +
      actions("ai", entry) +
      "</form>"
    );
  }

  function exchangeForm(entry) {
    const isNew = !entry;
    const envLive = !entry || entry.env !== "demo";
    return (
      '<form id="k-form-exchange" class="keys-form" autocomplete="off">' +
      '<div class="keys-form-title">' + esc(isNew ? t("keysNewExchange") : t("keysEditExchange")) + "</div>" +
      field(t("keysLblAlias"), '<input id="k-exchange-alias" class="sf-input" type="text" autocomplete="off" ' +
        'placeholder="' + esc(t("keysPhAlias")) + '" value="' + esc(entry ? entry.alias : "") + '">') +
      field(t("keysLblEnv"), '<select id="k-exchange-env" class="sf-input">' +
        '<option value="live"' + (envLive ? " selected" : "") + ">" + esc(t("keysEnvLive")) + "</option>" +
        '<option value="demo"' + (envLive ? "" : " selected") + ">" + esc(t("keysEnvDemo")) + "</option>" +
        "</select>") +
      field("API Key", '<input id="k-exchange-key" class="sf-input" type="password" autocomplete="new-password" ' +
        'spellcheck="false" placeholder="' + esc(entry ? t("keysPhKey") : "") + '">') +
      field("API Secret", '<input id="k-exchange-secret" class="sf-input" type="password" autocomplete="new-password" ' +
        'spellcheck="false" placeholder="' + esc(entry ? t("keysPhSecret") : "") + '">') +
      hintLine("API Key / Secret", entry ? tailPair(entry) : "") +
      actions("exchange", entry) +
      "</form>"
    );
  }

  /// 表单底部的一行提示(已保存的密钥只在这里露尾号, 输入框本身永远为空)。
  function hintLine(label, value) {
    if (!value) return "";
    return '<div class="keys-hint">' + esc(label) + ": " + esc(value) + "</div>";
  }

  function tailPair(entry) {
    return tailOf(entry.key) + " · " + tailOf(entry.secret);
  }

  /// 表单按钮组: 新增 = 保存 + 取消; 已存在 = 选用 + 保存 + 删除。
  /// 每个按钮都带 `data-kind`, 事件处理只认它 —— 不再靠"猜父元素是哪个表单"。
  function actions(kind, entry) {
    const k = ' data-kind="' + kind + '"';
    const b = '<button type="submit" class="keys-btn primary" data-act="save"' + k + ">" +
      esc(t("keysBtnSave")) + "</button>";
    if (!entry) {
      return '<div class="keys-actions">' + b +
        '<button type="button" class="keys-btn" data-act="cancel"' + k + ">" +
        esc(t("keysBtnCancel")) + "</button></div>";
    }
    const use = entry.active
      ? '<span class="keys-btn ghost" aria-disabled="true">' + esc(t("keysBadgeUsing")) + "</span>"
      : '<button type="button" class="keys-btn" data-act="use"' + k + ">" + esc(t("keysBtnUse")) + "</button>";
    return (
      '<div class="keys-actions">' + use + b +
      '<button type="button" class="keys-btn danger" data-act="delete"' + k + ">" +
      esc(t("keysBtnDelete")) + "</button></div>"
    );
  }

  // ---------- 当前生效摘要 ----------

  function aiCurrent(c) {
    const val = c.api_key.configured
      ? t("keysConfigured") + shortTail(c.api_key)
      : t("keysCurrentNone");
    const from = c.from_entry
      ? " " + t("keysFromEntry") + esc(c.from_entry) + t("keysFromEntryEnd")
      : "";
    return (
      '<div class="keys-current-row"><span class="keys-current-label">' + esc(t("keysCurrent")) + "</span>" +
      "<span>" + esc(c.provider) + (c.model ? " · " + esc(c.model) : "") + " · " + esc(val) + from + "</span>" +
      (c.api_key.configured
        ? '<button type="button" class="keys-btn danger" data-act="clear" data-target="ai">' +
          esc(t("keysClear")) + "</button>"
        : "") +
      "</div>"
    );
  }

  function exchangeCurrent(c) {
    const row = (label, secret, from, target) =>
      '<div class="keys-current-row"><span class="keys-current-label">' + esc(label) + "</span>" +
      "<span>" + esc(secret.configured ? t("keysConfigured") + shortTail(secret) : t("keysCurrentNone")) +
      (from ? " " + t("keysFromEntry") + esc(from) + t("keysFromEntryEnd") : "") + "</span>" +
      (secret.configured
        ? '<button type="button" class="keys-btn danger" data-act="clear" data-target="' + target + '">' +
          esc(t("keysClear")) + "</button>"
        : "") +
      "</div>";
    return (
      row(t("keysEnvLive"), c.binance_key, c.live_from, "live") +
      row(t("keysEnvDemo"), c.demo_key, c.demo_from, "demo")
    );
  }

  // ---------- 组装 ----------

  function card(titleKey, descKey, body) {
    return (
      '<section class="settings-card keys-card">' +
      '<div class="settings-card-head"><span class="settings-card-title">' + esc(t(titleKey)) + "</span></div>" +
      '<p class="settings-card-desc">' + esc(t(descKey)) + "</p>" +
      body + "</section>"
    );
  }

  function aiCard() {
    const g = data.ai;
    const items = g.entries.length
      ? g.entries.map(aiItem).join("")
      : '<li class="keys-empty">' + esc(t("keysEmptyAi")) + "</li>";
    const entry = entryOf("ai");
    const detail = draft.ai || entry ? aiForm(entry) : '<div class="keys-pick">' + esc(t("keysPick")) + "</div>";
    return card(
      "keysGroupAi",
      "keysDescAi",
      '<div class="keys-cols"><div class="keys-list-wrap">' +
        '<div class="keys-list-head"><span>' + esc(g.entries.length) + "</span>" +
        '<button type="button" class="keys-add" data-act="new" data-kind="ai">' + esc(t("keysAdd")) + "</button></div>" +
        '<ul class="keys-list">' + items + "</ul></div>" +
        '<div class="keys-detail">' + detail + "</div></div>" +
        '<div class="keys-current">' + aiCurrent(g.current) + "</div>",
    );
  }

  function exchangeCard() {
    const g = data.exchange;
    const items = g.entries.length
      ? g.entries.map(exchangeItem).join("")
      : '<li class="keys-empty">' + esc(t("keysEmptyExchange")) + "</li>";
    const entry = entryOf("exchange");
    const detail = draft.exchange || entry ? exchangeForm(entry) : '<div class="keys-pick">' + esc(t("keysPick")) + "</div>";
    return card(
      "keysGroupExchange",
      "keysDescExchange",
      '<div class="keys-cols"><div class="keys-list-wrap">' +
        '<div class="keys-list-head"><span>' + esc(g.entries.length) + "</span>" +
        '<button type="button" class="keys-add" data-act="new" data-kind="exchange">' + esc(t("keysAdd")) + "</button></div>" +
        '<ul class="keys-list">' + items + "</ul></div>" +
        '<div class="keys-detail">' + detail + "</div></div>" +
        '<div class="keys-current">' + exchangeCurrent(g.current) + "</div>",
    );
  }

  function render() {
    if (!data) return;
    $("view-keys").innerHTML =
      '<div class="keys-wrap">' +
      '<div class="settings-head keys-head"><h2 class="settings-title">' + esc(t("keysTitle")) + "</h2>" +
      '<p class="settings-lead">' + esc(t("keysLead")) + "</p></div>" +
      aiCard() +
      exchangeCard() +
      '<div id="keys-msg" class="settings-msg" aria-live="polite"></div>' +
      "</div>";
  }

  function showMsg(text, ok) {
    const msg = $("keys-msg");
    if (!msg) return;
    msg.textContent = text || "";
    msg.classList.toggle("ok", ok === true);
    msg.classList.toggle("err", ok === false);
    if (text) {
      const seq = ++msgSeq;
      // 成功的提示 4 秒后自清; 期间若又出了新提示(seq 变了)就不清, 免得擦掉更晚的那条。
      window.setTimeout(() => {
        if (seq === msgSeq && $("keys-msg")) {
          $("keys-msg").textContent = "";
          $("keys-msg").className = "settings-msg";
        }
      }, 4000);
    }
  }

  // ---------- 取数 ----------

  async function refresh() {
    data = await R.api("/api/keys");
    // 选中的条目可能已被删除或改名 → 落回未选状态。
    for (const kind of KINDS) {
      if (sel[kind] && !data[kind].entries.some((e) => e.alias === sel[kind])) sel[kind] = null;
    }
    render();
  }

  // ---------- 事件 ----------

  function kindOf(el) {
    return el && el.dataset ? el.dataset.kind : null;
  }

  /// 从列表项取条目(按渲染时的下标; 列表与 data 同源, 不会错位)。
  function itemOf(el) {
    const kind = kindOf(el);
    const i = Number(el.dataset.i);
    return kind && data && data[kind] ? data[kind].entries[i] || null : null;
  }

  async function onAction(ev) {
    const el = ev.target.closest("[data-act]");
    if (!el || busy) return;
    const act = el.dataset.act;

    if (act === "select") {
      const e = itemOf(el);
      if (!e) return;
      const kind = kindOf(el);
      sel[kind] = e.alias;
      draft[kind] = false;
      render();
      return;
    }

    if (act === "new") {
      const kind = kindOf(el);
      draft[kind] = true;
      sel[kind] = null;
      render();
      const first = $("k-" + kind + "-alias");
      if (first) first.focus();
      return;
    }

    if (act === "cancel") {
      const kind = kindOf(el);
      if (!kind) return;
      draft[kind] = false;
      render();
      return;
    }

    if (act === "use") {
      const kind = kindOf(el);
      const alias = sel[kind];
      if (!alias) return;
      await run(() => R.api("/api/keys/" + kind + "/use", { method: "POST", body: { alias: alias } }),
        t("keysUsing"), t("keysUseFailed"));
      return;
    }

    if (act === "delete") {
      const kind = kindOf(el);
      const alias = sel[kind];
      if (!alias || !window.confirm(t("keysConfirmDelete"))) return;
      draft[kind] = false;
      await run(() => R.api("/api/keys/" + kind + "/delete", { method: "POST", body: { alias: alias } }),
        t("keysDeleted"), t("keysDeleteFailed"));
      return;
    }

    if (act === "clear") {
      if (!window.confirm(t("keysConfirmClear"))) return;
      const target = el.dataset.target;
      await run(() => R.api("/api/keys/clear", { method: "POST", body: { target: target } }),
        t("keysCleared"), t("keysClearFailed"));
    }
  }

  /// 统一的"发请求 → 刷新 → 报结果"收尾; 失败时错误文案照实显示。
  async function run(req, okText, failPrefix) {
    busy = true;
    try {
      await req();
      await refresh();
      showMsg(okText, true);
    } catch (err) {
      showMsg(failPrefix + (err && err.message), false);
    } finally {
      busy = false;
    }
  }

  async function onSave(ev) {
    const form = ev.target;
    if (form.id !== "k-form-ai" && form.id !== "k-form-exchange") return;
    ev.preventDefault();
    if (busy) return;
    const isAi = form.id === "k-form-ai";

    if (isAi) {
      const alias = $("k-ai-alias").value.trim();
      if (!alias) return showMsg(t("keysNeedAlias"), false);
      const picker = $("k-ai-provider");
      const provider =
        picker.value === "__custom__" ? $("k-ai-provider-custom").value.trim() : picker.value;
      if (!provider) return showMsg(t("keysNeedProvider"), false);
      const body = {
        alias: alias,
        prev_alias: sel.ai || null,
        provider: provider,
        model: $("k-ai-model").value.trim(),
        base_url: $("k-ai-baseurl").value.trim(),
        api_key: $("k-ai-key").value,
      };
      // 成功后再把选中项指到新别名(改名后列表才跟得上)。
      busy = true;
      try {
        await R.api("/api/keys/ai/save", { method: "POST", body: body });
        sel.ai = alias;
        draft.ai = false;
        await refresh();
        showMsg(t("keysSaved"), true);
      } catch (err) {
        showMsg(t("keysSaveFailed") + (err && err.message), false);
      } finally {
        busy = false;
      }
      return;
    }

    const alias = $("k-exchange-alias").value.trim();
    if (!alias) return showMsg(t("keysNeedAlias"), false);
    const key = $("k-exchange-key").value;
    const secret = $("k-exchange-secret").value;
    // 新增时两者必填; 编辑时留空 = 保留原值(服务端再校一次成对性)。
    if (!sel.exchange && (!key || !secret)) return showMsg(t("keysNeedPair"), false);
    const body = {
      alias: alias,
      prev_alias: sel.exchange || null,
      env: $("k-exchange-env").value,
      key: key,
      secret: secret,
    };
    busy = true;
    try {
      await R.api("/api/keys/exchange/save", { method: "POST", body: body });
      sel.exchange = alias;
      draft.exchange = false;
      await refresh();
      showMsg(t("keysSaved"), true);
    } catch (err) {
      showMsg(t("keysSaveFailed") + (err && err.message), false);
    } finally {
      busy = false;
    }
  }

  /// 服务商换成预设时, 把该预设的推荐模型 / 接口地址填成 placeholder(不替用户写值)。
  function onChange(ev) {
    const el = ev.target;
    if (el.id !== "k-ai-provider") return;
    const custom = $("k-ai-provider-custom");
    const isCustom = el.value === "__custom__";
    if (custom) {
      custom.hidden = !isCustom;
      if (isCustom) custom.focus();
    }
    const preset = presetOf(el.value);
    const model = $("k-ai-model");
    const base = $("k-ai-baseurl");
    if (preset && model) model.placeholder = preset.model || t("keysPhModel");
    if (preset && base) base.placeholder = preset.base_url || t("keysPhBaseUrl");
  }

  function mount() {
    if (mounted) return;
    const host = $("view-keys");
    host.addEventListener("click", onAction);
    host.addEventListener("submit", onSave);
    host.addEventListener("change", onChange);
    mounted = true;
  }

  // 切语言时整页重画(动态文案占了这一页的绝大部分)。
  const baseApplyLang = R.applyLang;
  R.applyLang = function (lang) {
    const ret = baseApplyLang.call(this, lang);
    try {
      if (mounted && data) render();
    } catch (_) {
      // 视图尚未挂载时无 DOM 可改, 忽略。
    }
    return ret;
  };

  R.views.keys = {
    activate: async () => {
      mount();
      R.applyLang(R.lang);
      try {
        await refresh();
        showMsg("", null);
      } catch (err) {
        showMsg(t("keysLoadFailed") + (err && err.message), false);
      }
    },
  };
})();
