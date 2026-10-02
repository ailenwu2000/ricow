// Ricow Web 前端「设置」视图 (032 US1, 033 收敛): 只保留市场视野开关。
//
// 033 把密钥配置搬到了独立的「密钥」视图(`keys.js` / `#keys`), 那里按别名管理多套
// AI 与币安凭据。本页不再直接编辑任何密钥, 只留一张卡片把用户引过去 —— 免得同一件事
// 有两个入口、两套语义(旧入口"留空=不修改"无法删除, 容易留下幽灵凭据)。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  // ---------- 双语文案(静态标签走 data-zh/data-en) ----------
  Object.assign(R.TEXT.zh, {
    settingsTitle: "设置",
    settingsLead:
      "这里放通用偏好。币安凭据与 AI 通道的 API Key 请到左侧「密钥」页管理 —— 那里可以保存多套、按别名随时切换。",
    cardMarket: "市场视野",
    descMarket: "关闭时只显示 bStock 美股代币现货与股票永续; 打开后显示币安全部在售交易对。",
    lblShowAll: "显示全部交易对",
    cardKeys: "密钥",
    descKeys:
      "币安主网 / 测试网凭据、以及 AI 通道的 API Key, 都在「密钥」页按别名保存与切换;密钥只写入本机 ricow.toml。",
    goKeys: "前往密钥页",
    marketSaved: "视野设置已保存",
    marketFailed: "视野设置失败: ",
    loadFailed: "加载配置失败: ",
  });
  Object.assign(R.TEXT.en, {
    settingsTitle: "Settings",
    settingsLead:
      "General preferences live here. Binance credentials and the AI channel's API key are managed on the “Keys” page — keep several sets there and switch by alias.",
    cardMarket: "Market universe",
    descMarket:
      "Off: only bStock tokenized equities (spot) and equity perpetuals. On: every trading pair on Binance.",
    lblShowAll: "Show all trading pairs",
    cardKeys: "Keys",
    descKeys:
      "Binance mainnet / testnet credentials and the AI channel's API key are saved and switched by alias on the “Keys” page; values go only into your local ricow.toml.",
    goKeys: "Go to Keys",
    marketSaved: "Market view saved",
    marketFailed: "Failed to save market view: ",
    loadFailed: "Failed to load config: ",
  });

  let mounted = false;
  let lastData = null; // 最近一次 GET 响应
  let busy = false;

  const $ = (id) => document.getElementById(id);

  /// 静态双语 label(切语言时由 R.applyLang 统一换)。
  function bi(labelKey) {
    return (
      'data-zh="' + R.TEXT.zh[labelKey] + '" data-en="' + R.TEXT.en[labelKey] + '"'
    );
  }

  function card(cls, titleKey, descKey, body) {
    return (
      '<section class="settings-card ' + cls + '">' +
      '<div class="settings-card-head"><span class="settings-card-title" ' + bi(titleKey) + ">" +
      R.TEXT.zh[titleKey] + "</span></div>" +
      '<p class="settings-card-desc" ' + bi(descKey) + ">" + R.TEXT.zh[descKey] + "</p>" +
      '<div class="settings-grid">' + body + "</div></section>"
    );
  }

  /// 一次性挂载 DOM(只建一次; 反复激活只重新取数)。
  function mount() {
    if (mounted) return;
    const host = $("view-settings");
    host.innerHTML =
      '<div class="settings-wrap">' +
      '<div class="settings-head"><h2 class="settings-title" ' + bi("settingsTitle") + ">" +
      R.TEXT.zh.settingsTitle + "</h2>" +
      '<p class="settings-lead" ' + bi("settingsLead") + ">" + R.TEXT.zh.settingsLead + "</p></div>" +
      '<div class="settings-body">' +
      card(
        "card-market",
        "cardMarket",
        "descMarket",
        '<label class="sf sf-check"><input id="set-show-all" type="checkbox" />' +
          '<span ' + bi("lblShowAll") + ">" + R.TEXT.zh.lblShowAll + "</span></label>",
      ) +
      // 密钥入口: 只跳转, 不在本页编辑任何密钥。
      '<section class="settings-card card-keys-link">' +
      '<div class="settings-card-head"><span class="settings-card-title" ' + bi("cardKeys") + ">" +
      R.TEXT.zh.cardKeys + "</span></div>" +
      '<p class="settings-card-desc" ' + bi("descKeys") + ">" + R.TEXT.zh.descKeys + "</p>" +
      '<div class="settings-grid"><button type="button" id="settings-go-keys" class="settings-btn">' +
      '<span ' + bi("goKeys") + ">" + R.TEXT.zh.goKeys + "</span></button></div></section>" +
      '<div class="settings-actions"><span id="settings-msg" class="settings-msg" aria-live="polite"></span></div>' +
      "</div></div>";

    $("set-show-all").addEventListener("change", onToggleShowAll);
    $("settings-go-keys").addEventListener("click", () => R.navigate("keys"));
    mounted = true;
  }

  function showMsg(text, ok) {
    const msg = $("settings-msg");
    if (!msg) return;
    msg.textContent = text || "";
    msg.classList.toggle("ok", ok === true);
    msg.classList.toggle("err", ok === false);
  }

  /// GET 填充: 只看市场视野这一个开关。
  async function refresh() {
    const d = await R.api("/api/config/keys");
    lastData = d;
    $("set-show-all").checked = d.market && d.market.show_all_pairs === true;
  }

  /// "显示全部交易对": change 即写盘; 失败回滚勾选并给出原因。
  async function onToggleShowAll() {
    if (busy) return;
    const cb = $("set-show-all");
    const wanted = cb.checked;
    busy = true;
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
    } finally {
      busy = false;
    }
  }

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
