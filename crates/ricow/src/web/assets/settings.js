// Ricow Web 前端「设置」视图 (032 US1): 币安主网/测试网凭据、AI 通道、市场视野开关。
// 注册为 `R.views.settings`; 所有读写经 R.api 走 `/api/config/keys`。
//
// 安全约束(FR-007 / FR-008 / SC-007):
// - GET 只拿到 configured + 尾号 hint, 密钥全文从不在前端出现;
// - 密钥输入框永远以空值挂载, 留空提交 = 不修改该项;
// - 不写任何浏览器持久存储, 密钥只随 POST 体进本机 ricow.toml。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  // ---------- 双语文案(并入公共字典, 静态标签走 data-zh/data-en) ----------
  Object.assign(R.TEXT.zh, {
    settingsLead:
      "密钥只保存在本机配置文件 ricow.toml(权限仅当前用户可读);页面不回显已保存的全文,密钥项留空保存表示不修改。",
    cardBinance: "币安主网凭据",
    descBinance: "用于实盘交易。建议只开交易权限、关闭提现,优先使用子账户或受限 Key。",
    cardDemo: "币安测试网凭据",
    descDemo: "仅用于 demo(测试网)模式;留空不影响对话与其余功能,只是无法以 demo 模式启动策略。",
    cardAi: "AI 助手",
    descAi:
      "服务商可写内置预设名或自定义名;自建/中转端点请同时填写接口地址。所选模型须支持工具调用(function calling)。",
    cardMarket: "市场视野",
    descMarket: "关闭时只显示 bStock 美股代币现货与股票永续;打开后显示币安全部在售交易对。",
    lblKey: "API Key",
    lblSecret: "API Secret",
    lblProvider: "服务商 (provider)",
    lblModel: "模型 (model)",
    lblBaseUrl: "接口地址 (base_url)",
    lblAiKey: "AI API Key",
    lblShowAll: "显示全部交易对",
    demoBadge: "可选",
    phProvider: "如 deepseek,或自定义端点名",
    phModel: "如 deepseek-chat",
    phBaseUrl: "自建/中转端点才需要,如 https://api.example.com/v1",
    secretConfigured: "已配置 · 尾号 ",
    secretNotSet: "未配置",
    save: "保存",
    saving: "保存中…",
    saved: "已保存",
    saveFailed: "保存失败: ",
    loadFailed: "加载配置失败: ",
    marketSaved: "视野设置已保存",
    marketFailed: "视野设置失败: ",
  });
  Object.assign(R.TEXT.en, {
    settingsLead:
      "Keys are stored only in the local ricow.toml (readable by your user alone). Full values are never shown back; leave a key field empty on save to keep it unchanged.",
    cardBinance: "Binance mainnet credentials",
    descBinance:
      "Used for live trading. Enable trading only, disable withdrawals, and prefer a sub-account or restricted key.",
    cardDemo: "Binance testnet credentials",
    descDemo:
      "Only used for demo (testnet) mode. Leaving them empty does not affect chat or other features; strategies just cannot start in demo mode.",
    cardAi: "AI assistant",
    descAi:
      "Use a built-in preset name or a custom provider name; fill the base URL for self-hosted/proxy endpoints. The model must support function calling.",
    cardMarket: "Market universe",
    descMarket:
      "Off: only bStock tokenized equities (spot) and equity perpetuals. On: every trading pair on Binance.",
    lblKey: "API Key",
    lblSecret: "API Secret",
    lblProvider: "Provider",
    lblModel: "Model",
    lblBaseUrl: "Base URL",
    lblAiKey: "AI API Key",
    lblShowAll: "Show all trading pairs",
    demoBadge: "Optional",
    phProvider: "e.g. deepseek, or a custom endpoint name",
    phModel: "e.g. deepseek-chat",
    phBaseUrl: "Only for custom/proxy endpoints, e.g. https://api.example.com/v1",
    secretConfigured: "Configured · ends with ",
    secretNotSet: "Not configured",
    save: "Save",
    saving: "Saving…",
    saved: "Saved",
    saveFailed: "Save failed: ",
    loadFailed: "Failed to load config: ",
    marketSaved: "Market view saved",
    marketFailed: "Failed to save market view: ",
  });

  // 密钥字段: id → (section, key, 从 GET 响应取脱敏视图)。
  const SECRETS = [
    { id: "set-binance-key", section: "exchange", key: "binance_key", pick: (d) => d.binance_key },
    { id: "set-binance-secret", section: "exchange", key: "binance_secret", pick: (d) => d.binance_secret },
    { id: "set-demo-key", section: "exchange", key: "demo_key", pick: (d) => d.demo_key },
    { id: "set-demo-secret", section: "exchange", key: "demo_secret", pick: (d) => d.demo_secret },
    { id: "set-ai-key", section: "ai", key: "api_key", pick: (d) => d.ai.api_key },
  ];
  // 非密钥文本字段: 原值回填, 空串保存 = 清空。
  const TEXT_FIELDS = [
    { id: "set-ai-provider", section: "ai", key: "provider", pick: (d) => d.ai.provider },
    { id: "set-ai-model", section: "ai", key: "model", pick: (d) => d.ai.model },
    { id: "set-ai-baseurl", section: "ai", key: "base_url", pick: (d) => d.ai.base_url },
  ];

  let mounted = false;
  let lastData = null; // 最近一次 GET 响应(切语言时重画动态 placeholder 用)
  let saving = false;

  const $ = (id) => document.getElementById(id);

  /// 单个密钥输入框(label + password input)。
  function secretField(id, labelKey) {
    return (
      '<label class="sf"><span data-zh="' + R.TEXT.zh[labelKey] + '" data-en="' +
      R.TEXT.en[labelKey] + '">' + R.TEXT.zh[labelKey] + "</span>" +
      '<input id="' + id + '" class="sf-input" type="password" autocomplete="new-password" ' +
      'spellcheck="false" /></label>'
    );
  }

  /// 普通文本输入框(label + text input, 静态 placeholder 随语言切换)。
  function textField(id, labelKey, phKey) {
    return (
      '<label class="sf"><span data-zh="' + R.TEXT.zh[labelKey] + '" data-en="' +
      R.TEXT.en[labelKey] + '">' + R.TEXT.zh[labelKey] + "</span>" +
      '<input id="' + id + '" class="sf-input" type="text" autocomplete="off" spellcheck="false" ' +
      'data-zh-placeholder="' + R.TEXT.zh[phKey] + '" data-en-placeholder="' +
      R.TEXT.en[phKey] + '" /></label>'
    );
  }

  function card(cls, titleKey, descKey, body, badgeKey) {
    const badge = badgeKey
      ? ' <span class="settings-badge" data-zh="' + R.TEXT.zh[badgeKey] + '" data-en="' +
        R.TEXT.en[badgeKey] + '">' + R.TEXT.zh[badgeKey] + "</span>"
      : "";
    return (
      '<section class="settings-card ' + cls + '">' +
      '<div class="settings-card-head"><span class="settings-card-title" data-zh="' +
      R.TEXT.zh[titleKey] + '" data-en="' + R.TEXT.en[titleKey] + '">' + R.TEXT.zh[titleKey] +
      "</span>" + badge + "</div>" +
      '<p class="settings-card-desc" data-zh="' + R.TEXT.zh[descKey] + '" data-en="' +
      R.TEXT.en[descKey] + '">' + R.TEXT.zh[descKey] + "</p>" +
      '<div class="settings-grid">' + body + "</div></section>"
    );
  }

  /// 一次性挂载 DOM(只建一次; 反复激活只重新取数)。
  function mount() {
    if (mounted) return;
    const host = $("view-settings");
    host.innerHTML =
      '<div class="settings-wrap">' +
      '<div class="settings-head"><h2 class="settings-title" data-zh="设置" data-en="Settings">设置</h2>' +
      '<p class="settings-lead" data-zh="' + R.TEXT.zh.settingsLead + '" data-en="' +
      R.TEXT.en.settingsLead + '">' + R.TEXT.zh.settingsLead + "</p></div>" +
      '<form id="settings-form" class="settings-body" autocomplete="off">' +
      card(
        "card-binance",
        "cardBinance",
        "descBinance",
        secretField("set-binance-key", "lblKey") + secretField("set-binance-secret", "lblSecret"),
      ) +
      card(
        "card-demo",
        "cardDemo",
        "descDemo",
        secretField("set-demo-key", "lblKey") + secretField("set-demo-secret", "lblSecret"),
        "demoBadge",
      ) +
      card(
        "card-ai",
        "cardAi",
        "descAi",
        textField("set-ai-provider", "lblProvider", "phProvider") +
          textField("set-ai-model", "lblModel", "phModel") +
          textField("set-ai-baseurl", "lblBaseUrl", "phBaseUrl") +
          secretField("set-ai-key", "lblAiKey"),
      ) +
      card(
        "card-market",
        "cardMarket",
        "descMarket",
        '<label class="sf sf-check"><input id="set-show-all" type="checkbox" />' +
          '<span data-zh="' + R.TEXT.zh.lblShowAll + '" data-en="' + R.TEXT.en.lblShowAll + '">' +
          R.TEXT.zh.lblShowAll + "</span></label>",
      ) +
      '<div class="settings-actions"><button id="settings-save" type="submit" data-zh="' +
      R.TEXT.zh.save + '" data-en="' + R.TEXT.en.save + '">' + R.TEXT.zh.save + "</button>" +
      '<span id="settings-msg" class="settings-msg" aria-live="polite"></span></div>' +
      "</form></div>";

    $("settings-form").addEventListener("submit", onSave);
    $("set-show-all").addEventListener("change", onToggleShowAll);
    mounted = true;
  }

  /// 按当前语言 + 最近一次 GET 的脱敏状态重画密钥框 placeholder(不碰输入值)。
  function applySecretPlaceholders() {
    if (!mounted || !lastData) return;
    for (const f of SECRETS) {
      const input = $(f.id);
      if (!input) continue;
      const info = f.pick(lastData) || {};
      if (info.configured && info.hint) {
        // hint 形如 "***a1b2"; 过短密钥服务端固定给 "****"(不泄露长度), 原样拼上。
        const tail = info.hint.length > 4 ? info.hint.slice(3) : info.hint;
        input.placeholder = t("secretConfigured") + tail;
      } else {
        input.placeholder = t("secretNotSet");
      }
    }
  }

  function showMsg(text, ok) {
    const msg = $("settings-msg");
    msg.textContent = text || "";
    msg.classList.toggle("ok", ok === true);
    msg.classList.toggle("err", ok === false);
  }

  /// GET 填充: 文本字段原值; 密钥框留空只给脱敏 placeholder; 复选框按值预选。
  async function refresh() {
    const d = await R.api("/api/config/keys");
    lastData = d;
    for (const f of TEXT_FIELDS) $(f.id).value = f.pick(d) || "";
    for (const f of SECRETS) $(f.id).value = "";
    applySecretPlaceholders();
    $("set-show-all").checked = d.market && d.market.show_all_pairs === true;
  }

  /// [保存]: 文本字段全量提交(空串=清空), 密钥只提交非空项; 成功后重新 GET 刷新尾号提示。
  async function onSave(ev) {
    ev.preventDefault();
    if (saving) return;
    const updates = [];
    for (const f of TEXT_FIELDS) {
      updates.push({ section: f.section, key: f.key, value: $(f.id).value });
    }
    for (const f of SECRETS) {
      const v = $(f.id).value;
      if (v) updates.push({ section: f.section, key: f.key, value: v });
    }
    saving = true;
    $("settings-save").disabled = true;
    $("settings-save").textContent = t("saving");
    try {
      await R.api("/api/config/keys", { method: "POST", body: { updates: updates } });
      showMsg(t("saved"), true);
      await refresh();
    } catch (err) {
      showMsg(t("saveFailed") + (err && err.message), false);
    } finally {
      saving = false;
      $("settings-save").disabled = false;
      $("settings-save").textContent = t("save");
    }
  }

  /// "显示全部交易对": change 即写盘; 失败回滚勾选并给出原因。
  async function onToggleShowAll() {
    const cb = $("set-show-all");
    const wanted = cb.checked;
    try {
      await R.api("/api/config/keys", {
        method: "POST",
        body: { updates: [{ section: "market", key: "show_all_pairs", value_bool: wanted }] },
      });
      if (lastData) lastData.market.show_all_pairs = wanted;
      showMsg(t("marketSaved"), true);
    } catch (err) {
      cb.checked = !wanted;
      showMsg(t("marketFailed") + (err && err.message), false);
    }
  }

  // 切语言时动态 placeholder 也要跟着换: 包一层公共 applyLang(对话视图切语言也走它)。
  const baseApplyLang = R.applyLang;
  R.applyLang = function (lang) {
    const ret = baseApplyLang.call(this, lang);
    try {
      applySecretPlaceholders();
    } catch (_) {
      // 设置视图尚未挂载时无 DOM 可改, 忽略。
    }
    return ret;
  };

  R.views.settings = {
    activate: async () => {
      mount();
      R.applyLang(R.lang); // 静态标签立即按当前语言渲染
      try {
        await refresh();
        showMsg("", null);
      } catch (err) {
        showMsg(t("loadFailed") + (err && err.message), false);
      }
    },
  };
})();
