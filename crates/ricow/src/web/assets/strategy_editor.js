// Ricow Web 前端「策略」视图 — Lua 编辑器与保存 (042 从 strategies.js 拆出):
//   语法高亮(overlay 双层) / 脏状态与撤销 / Tab 缩进 / 保存(编译门禁 + 运行中 409 锁) /
//   AI 改写(草稿 + need_keys 引导) / 静态检查提示。
// 取用: 工具与共享状态来自 `R.sg`; 跨文件符号见本文件末尾的 `S.xxx =`。
// 本文件**不做**: 详情表单渲染(在 strategy_form.js)、回测(在 strategy_backtest.js)。
"use strict";

(function () {
  const R = window.Ricow;
  const S = R.sg;
  const t = S.t;
  const $ = S.$;
  const h = S.h;
  const setMsg = S.setMsg;
  const st = S.st;

  // 跨文件符号(042 D2: 延迟查找)
  const gatherParams = (...a) => S.gatherParams(...a);
  const enterDetail = (...a) => S.enterDetail(...a);

  // ---------- 编辑器脏状态 / 高亮 / Tab / 撤销 (P1-4) ----------

  /// Lua 语法高亮(单遍正则 tokenizer): 注释/字符串/数字/关键字/ctx·exec API。
  /// 内容经 escape 后才进 innerHTML —— 用户与 AI 的代码都不可信, 只以纯文本形态着色。
  function highlightLua(src) {
    const esc = (s) =>
      s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const re =
      /(--\[\[[\s\S]*?\]\]|--[^\n]*)|("(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*')|\b(0x[0-9a-fA-F]+|\d+\.?\d*)\b|\b(function|end|if|then|elseif|else|for|while|do|return|local|and|or|not|nil|true|false|break|repeat|until|in)\b|(ctx:\w+|exec\.\w+)/g;
    let out = "";
    let last = 0;
    let m;
    while ((m = re.exec(src))) {
      out += esc(src.slice(last, m.index));
      const full = m[0];
      if (m[1]) out += '<span class="hl-com">' + esc(full) + "</span>";
      else if (m[2]) out += '<span class="hl-str">' + esc(full) + "</span>";
      else if (m[3]) out += '<span class="hl-num">' + esc(full) + "</span>";
      else if (m[4]) out += '<span class="hl-kw">' + esc(full) + "</span>";
      else out += '<span class="hl-api">' + esc(full) + "</span>";
      last = m.index + full.length;
    }
    out += esc(src.slice(last));
    // 末尾补一个换行: pre 与 textarea 的滚动高度对齐(软换行场景的宽高一致性)。
    return out + "\n";
  }

  /// 把 textarea 当前内容同步进高亮层(内容与滚动双同步)。
  function refreshHighlight() {
    const ta = $("sg-code");
    const hl = $("sg-hl");
    if (!ta || !hl) return;
    hl.innerHTML = highlightLua(ta.value);
    hl.scrollTop = ta.scrollTop;
    hl.scrollLeft = ta.scrollLeft;
  }

  /// 编译错误行定位: 把光标移到该行并选中, 编辑器滚到可见。
  function jumpToErrorLine(line) {
    if (typeof line !== "number" || line < 1) return;
    const ta = $("sg-code");
    const lines = ta.value.split("\n");
    if (line > lines.length) return;
    let start = 0;
    for (let i = 0; i < line - 1; i++) start += lines[i].length + 1;
    ta.focus();
    ta.setSelectionRange(start, start + lines[line - 1].length);
    // 粗略滚动到该行(行高一致, 由 CSS 保证)。
    const lh = parseFloat(getComputedStyle(ta).lineHeight) || 20;
    ta.scrollTop = Math.max(0, (line - 5) * lh);
    refreshHighlight();
  }

  /// 静态检查提示(P1-5): 黄色提示列表, 只提示不拦截。
  function renderWarnings(list) {
    const box = $("sg-warnlist");
    box.innerHTML = "";
    if (!list || !list.length) {
      box.hidden = true;
      return;
    }
    box.appendChild(h("span", "sg-warnlist-title", t("sgWarnings")));
    const ul = h("ul", "sg-warnlist-ul");
    for (const w of list) ul.appendChild(h("li", null, w));
    box.appendChild(ul);
    box.hidden = false;
  }

  function updateDirty() {
    const codeEl = $("sg-code");
    const dirty = codeEl.value !== st.savedCode;
    $("sg-dirty").hidden = !dirty;
    $("sg-dirty-text").textContent = st.aiDraft ? t("sgAiDirty") : t("sgDirty");
  }

  function onCodeKeydown(ev) {
    if (ev.key !== "Tab") return;
    // Tab 插入两空格并保持选区, 不跳焦(代码编辑器基本手感)。
    ev.preventDefault();
    const el = ev.target;
    const start = el.selectionStart;
    const end = el.selectionEnd;
    el.value = el.value.slice(0, start) + "  " + el.value.slice(end);
    el.selectionStart = el.selectionEnd = start + 2;
    updateDirty();
  }

  function onUndo() {
    $("sg-code").value = st.savedCode;
    st.aiDraft = false;
    setMsg($("sg-savemsg"), "", "");
    renderWarnings([]);
    updateDirty();
    refreshHighlight();
  }

  // ---------- 保存 ----------

  function syncSaveBtn(mode) {
    for (const btn of [$("sg-save"), $("sg-save-top")]) {
      if (!btn) continue;
      if (mode === "saving") {
        btn.disabled = true;
        btn.textContent = t("sgSaving");
      } else if (mode === "locked") {
        btn.disabled = true;
        btn.textContent = t("sgSaveLocked");
      } else {
        btn.disabled = false;
        btn.textContent = t("sgSave");
      }
    }
  }

  function compileText(err) {
    // 编译错误: 服务端给 chunk 行号时定位到"第 N 行", 原文(中文 mlua 输出)照贴。
    const where = typeof err.line === "number" ? "第 " + err.line + " 行: " : "";
    return where + ((err && err.message) || "");
  }

  async function onSave() {
    if (!st.detailCtx || st.saveLocked) return;
    const code = $("sg-code").value;
    const pair = $("sg-pair").value.trim();
    if (!pair) {
      setMsg($("sg-savemsg"), t("sgPairRequired"), "err");
      return;
    }
    if (!code.trim()) {
      setMsg($("sg-savemsg"), t("sgCodeEmpty"), "err");
      return;
    }
    let params;
    try {
      params = gatherParams(true);
    } catch (err) {
      setMsg($("sg-savemsg"), err.message, "err");
      return;
    }

    syncSaveBtn("saving");
    setMsg($("sg-savemsg"), "", "");
    try {
      // 详情里的保存恒为覆盖自己(用户策略); 编译门禁 + 备份都在服务端。
      const reply = await R.api("/api/strategies", {
        method: "POST",
        body: {
          name: st.detailCtx.id,
          market: st.detailCtx.detail.market,
          pair: pair,
          code: code,
          params: params,
          overwrite: true,
        },
      });
      st.savedCode = code;
      st.aiDraft = false;
      updateDirty();
      refreshHighlight();
      setMsg(
        $("sg-savemsg"),
        reply && reply.warnings && reply.warnings.length ? t("sgSavedWithWarn") : t("sgSaved"),
        "ok"
      );
      renderWarnings(reply && reply.warnings);
      syncSaveBtn("idle");
      refreshListQuietly();
    } catch (err) {
      syncSaveBtn("idle");
      if (err && err.code === "running") {
        // FR-020: 运行中禁止覆盖 —— 锁保存钮, 给重载入口(停止后回来自动恢复)。
        st.saveLocked = true;
        syncSaveBtn("locked");
        showRunningHint(err.message || t("sgSaveLocked"));
      } else if (err && err.code === "compile") {
        setMsg($("sg-savemsg"), t("sgCompilePrefix") + " " + compileText(err), "err");
        jumpToErrorLine(err.line);
      } else {
        setMsg($("sg-savemsg"), (err && err.message) || String(err), "err");
      }
    }
  }

  /// 运行中提示条: 服务端中文原因 + 重新载入钮(用户停止策略后点它解除锁定)。
  function showRunningHint(message) {
    const el = $("sg-savemsg");
    el.innerHTML = "";
    el.className = "sg-msg err";
    el.appendChild(h("span", null, message + " "));
    const reload = h("button", "sg-btn sg-btn-mini", t("sgReload"));
    reload.type = "button";
    reload.addEventListener("click", () => {
      st.saveLocked = false;
      const g = st.gen;
      enterDetail(st.detailCtx.id, g);
    });
    el.appendChild(reload);
  }

  /// 保存成功后静默刷新列表缓存(不改当前视图; 过期代际忽略)。
  function refreshListQuietly() {
    const g = st.gen;
    R.api("/api/strategies")
      .then((data) => {
        if (g === st.gen) st.listData = data;
      })
      .catch(() => {});
  }

  // ---------- AI 改 Lua ----------

  function syncAiBtn(busy) {
    const btn = $("sg-ai-go");
    btn.disabled = !!busy;
    btn.textContent = busy ? t("sgAiBusy") : t("sgAiGo");
  }

  async function onAiEdit() {
    if (!st.detailCtx) return;
    const instruction = $("sg-ai-instr").value.trim();
    if (!instruction) {
      setMsg($("sg-ai-msg"), t("sgAiPh"), "err");
      return;
    }
    const g = st.gen;
    syncAiBtn(true);
    setMsg($("sg-ai-msg"), t("sgAiBusy"), "");
    try {
      const reply = await R.api(
        "/api/strategies/" + encodeURIComponent(st.detailCtx.id) + "/ai-edit",
        { method: "POST", body: { instruction: instruction } }
      );
      if (g !== st.gen) return;
      // 成功: 服务端已过编译门禁; 替换编辑器, 撤销可回上次保存版本。
      $("sg-code").value = reply.code;
      st.aiDraft = true;
      updateDirty();
      refreshHighlight();
      setMsg($("sg-ai-msg"), t("sgAiOk"), "ok");
      renderWarnings(reply.warnings);
    } catch (err) {
      if (g !== st.gen) return;
      if (err && err.status === 403 && err.code === "need_keys") {
        showNeedKeys(err.message || t("sgNeedKeys"));
      } else if (err && err.code === "compile") {
        // 编译没过: 服务端没给可⽤代码, 编辑器内容**不替换**。
        setMsg($("sg-ai-msg"), t("sgAiCompilePrefix") + " " + compileText(err), "err");
        jumpToErrorLine(err.line);
      } else {
        setMsg($("sg-ai-msg"), (err && err.message) || String(err), "err");
      }
    } finally {
      if (g === st.gen) syncAiBtn(false);
    }
  }

  /// need_keys: 文案 + 一键去密钥页(配置完用浏览器返回即可回到本视图)。
  function showNeedKeys(message) {
    const el = $("sg-ai-msg");
    el.innerHTML = "";
    el.className = "sg-msg err";
    el.appendChild(h("span", null, message + " "));
    const go = h("button", "sg-btn sg-btn-mini", t("sgGoSettings"));
    go.type = "button";
    go.addEventListener("click", () => R.navigate("keys"));
    el.appendChild(go);
  }

  // ---------- 对外(外壳与其它子文件调用) ----------
  S.updateDirty = updateDirty;
  S.refreshHighlight = refreshHighlight;
  S.syncSaveBtn = syncSaveBtn;
  S.syncAiBtn = syncAiBtn;
  S.onCodeKeydown = onCodeKeydown;
  S.onSave = onSave;
  S.onAiEdit = onAiEdit;
  // `onUndo` 也是外壳 `mount()` 直接挂到按钮上的回调(`$("sg-undo").addEventListener`),
  // 漏了这条导出 = 点开策略页整片 `ReferenceError: onUndo is not defined`(2026-10-07 真机走查抓到)。
  S.onUndo = onUndo;
})();
