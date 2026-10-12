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
  // N3-B: 两次回测 + 评估的编排在回测子文件里(它管 `/api/backtest` 的作业语义)。
  const runParamAssess = (...a) => S.runParamAssess(...a);

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
      // N3-B: "保存成功了"就等于"实例 TOML 现在是这些值" —— 把 detailCtx 里的当前值同步过去。
      // 不同步的话, 评估面板的「改前」会一直是**进页面时**读到的那份(可能是好几轮改动之前),
      // 对比出来的数字看着像实测, 其实比错了对象。
      if (st.detailCtx && st.detailCtx.detail) {
        st.detailCtx.detail.current = params;
        st.detailCtx.detail.pair = pair;
      }
      updateDirty();
      refreshHighlight();
      // B6: 成功走**全局 toast** —— 在详情页保存完回列表时, 提示不至于随详情 DOM 一起消失
      // (保存钮本身的"已变灰/未改动"状态也在, 但它说不清"刚才那次到底成没成")。
      const savedMsg =
        reply && reply.warnings && reply.warnings.length ? t("sgSavedWithWarn") : t("sgSaved");
      R.toast(savedMsg, "ok");
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

  // ---------- AI 改策略(N3: 多轮对话 + 说意图) ----------
  //
  // 两种模式:
  //   「调整参数」= 多轮对话。用户只说**想要的效果**(「保守点」「更看好震荡行情」), 不指定键与值 ——
  //                后端把它翻译成**一整组**参数取值, 并回一段讲清理由与风险的说明 + 逐键的前后对照;
  //                前端只填参数表单, 代码一个字不动。
  //   「改代码逻辑」= 原有的整份代码改写(草稿 + 编译门禁)。**仍是单轮**: 它每轮产出的是完整
  //                代码, 上一轮那版已经在编辑器里了, 再叠历史没有增量信息(见后端 ai_edit_strategy)。
  //
  // 历史**只存在前端内存**(`st.aiThreads`): 服务端端点是纯一次问答, 不引入会话状态 ——
  // 代价就是刷新即失, 所以卡片上明写这一点。回传时按 `用户: … / AI: …` 排成纯文本。
  //
  // 落盘时机两种模式完全一致: 都只是草稿, 用户点「保存」才写盘。

  /// 当前模式的对话线程(数组引用, 可原地 push)。
  function thread() {
    return st.aiThreads[st.aiMode === "code" ? "code" : "params"];
  }

  /// 把线程排成给模型的纯文本历史(不含本轮刚说的那句 —— 它单独走 `instruction`)。
  /// 后端还会再做一次字符上限截断([`cap_history`]), 这里不做二次裁剪: 让"最近多少轮"这件事
  /// 只有一个判据, 免得两处各截一半、结果谁也看不懂。
  function historyText(mode) {
    const turns = st.aiThreads[mode === "code" ? "code" : "params"];
    const lines = [];
    for (const turn of turns) {
      if (turn.role === "user") lines.push(t("sgAiUserLabel") + ": " + turn.text);
      else {
        if (turn.advice) lines.push(t("sgAiBotLabel") + ": " + turn.advice);
        if (turn.changed && turn.changed.length) {
          lines.push(
            t("sgAiBotLabel") + " " + t("sgAiChangedTitle") + ": " +
              turn.changed.map((c) => c.key + " " + (c.before || "—") + "→" + c.after).join("; ")
          );
        }
        if (turn.note) lines.push(t("sgAiBotLabel") + ": " + turn.note);
      }
    }
    return lines.join("\n");
  }

  /// 重画对话线程(纯 DOM, 数据全部来自 `st.aiThreads`)。
  function renderThread() {
    const box = $("sg-ai-thread");
    if (!box) return;
    box.innerHTML = "";
    const turns = thread();
    if (!turns.length) {
      box.hidden = true;
      return;
    }
    box.hidden = false;
    for (const turn of turns) box.appendChild(turnBubble(turn));
    // 新消息总在最下面 —— 不自动滚的话, 用户看到的是最旧那条, 会以为 AI 没回。
    if (box.scrollIntoView) box.lastChild.scrollIntoView({ block: "nearest" });
  }

  function turnBubble(turn) {
    const isUser = turn.role === "user";
    const wrap = h("div", "sg-ai-turn " + (isUser ? "sg-ai-turn-user" : "sg-ai-turn-bot"));
    wrap.appendChild(h("span", "sg-ai-who", isUser ? t("sgAiUserLabel") : t("sgAiBotLabel")));
    if (turn.error) {
      wrap.appendChild(h("div", "sg-ai-body sg-msg err", turn.error));
      return wrap;
    }
    if (turn.text) wrap.appendChild(h("div", "sg-ai-body", turn.text));
    if (turn.advice) wrap.appendChild(h("div", "sg-ai-body", turn.advice));
    if (turn.note) wrap.appendChild(h("div", "sg-ai-body sg-ai-note", turn.note));
    if (isUser) return wrap;

    // 参数模式: 逐键前后对照(只列**真变了**的键; 后端算好的, 前端不重算)。
    if (turn.changed) {
      if (!turn.changed.length) {
        wrap.appendChild(h("div", "sg-ai-body sg-ai-changed-none", t("sgAiChangedNone")));
      } else {
        const list = h("div", "sg-ai-changed");
        list.appendChild(h("div", "sg-ai-changed-title", t("sgAiChangedTitle")));
        for (const c of turn.changed) {
          const row = h("div", "sg-ai-change-row");
          row.appendChild(h("span", "sg-ai-change-name", c.name + " (" + c.key + ")"));
          const before = c.before === undefined || c.before === null ? "—" : String(c.before);
          row.appendChild(h("span", "sg-ai-change-num", before + " → " + c.after));
          if (c.before_is_default) {
            row.appendChild(h("span", "sg-ai-change-tag", t("sgAiChangedDefault")));
          }
          list.appendChild(row);
        }
        wrap.appendChild(list);
      }
    }
    if (turn.warnings && turn.warnings.length) {
      const ul = h("ul", "sg-warnlist-ul");
      for (const w of turn.warnings) ul.appendChild(h("li", null, w));
      wrap.appendChild(ul);
    }
    return wrap;
  }

  /// 提交一轮对话。两个模式的差异只在这一个函数里:
  /// 参数模式带 history 且回包带 advice/changed; 代码模式单轮, 回包是代码。
  async function onAiEdit() {
    if (!st.detailCtx) return;
    const instruction = $("sg-ai-instr").value.trim();
    if (!instruction) {
      setMsg($("sg-ai-msg"), t("sgAiEmpty"), "err");
      return;
    }
    const mode = st.aiMode === "code" ? "code" : "params";
    const g = st.gen;
    // 历史要在**推入本轮**之前取 —— 本轮的话单独走 `instruction`, 混进历史会被说两遍。
    const history = historyText(mode);
    thread().push({ role: "user", text: instruction });
    renderThread();
    $("sg-ai-instr").value = "";
    syncAiBtn(true);
    setMsg($("sg-ai-msg"), t("sgAiBusy"), "");
    try {
      const body = { instruction: instruction, mode: mode };
      if (mode === "params" && history) body.history = history;
      const reply = await R.api(
        "/api/strategies/" + encodeURIComponent(st.detailCtx.id) + "/ai-edit",
        { method: "POST", body: body }
      );
      if (g !== st.gen) return;
      if (mode === "params") {
        // 只填参数表单: 代码不动, 用户核对后点「保存」才写盘。
        const filled = fillParamInputs(reply.params || {});
        thread().push({
          role: "ai",
          advice: reply.advice || "",
          changed: reply.changed || [],
          warnings: reply.warnings || [],
          // 一个键都没填上 = 建议全落在表单之外: 这与"改了但没变"是两件事, 分开说。
          note: filled ? t("sgAiParamsOk") : t("sgAiParamsEmpty"),
        });
        renderThread();
        if (!reply.advice && !(reply.changed || []).length && !filled) {
          setMsg($("sg-ai-msg"), t("sgAiParamsEmpty"), "err");
        } else {
          setMsg($("sg-ai-msg"), "", "");
        }
        syncAssessPanel();
        return;
      }
      // code 模式: 服务端已过编译门禁; 替换编辑器, 撤销可回上次保存版本。
      $("sg-code").value = reply.code;
      st.aiDraft = true;
      updateDirty();
      refreshHighlight();
      thread().push({ role: "ai", note: t("sgAiOk"), warnings: reply.warnings || [] });
      renderThread();
      setMsg($("sg-ai-msg"), t("sgAiOk"), "ok");
      renderWarnings(reply.warnings);
    } catch (err) {
      if (g !== st.gen) return;
      // 失败也进线程: 否则对话看起来像"用户的话没人接", 而且失败原因(比如清单为空、
      // 没配密钥)正是下一句要接着说的上下文。
      const message = (err && err.message) || String(err);
      thread().push({ role: "ai", error: message });
      renderThread();
      if (err && err.status === 403 && err.code === "need_keys") {
        showNeedKeys(message);
      } else if (err && err.code === "compile") {
        // 编译没过: 服务端没给可⽤代码, 编辑器内容**不替换**。
        setMsg($("sg-ai-msg"), t("sgAiCompilePrefix") + " " + compileText(err), "err");
        jumpToErrorLine(err.line);
      } else {
        setMsg($("sg-ai-msg"), message, "err");
      }
    } finally {
      if (g === st.gen) syncAiBtn(false);
    }
  }

  /// N3: 清空当前模式的对话(历史全在前端, 服务端没有可清的东西)。
  function onAiClearThread() {
    st.aiThreads[st.aiMode === "code" ? "code" : "params"] = [];
    renderThread();
    setMsg($("sg-ai-msg"), "", "");
    syncAssessPanel();
  }

  // ---------- N3-B: 改完自动评估影响 ----------
  //
  // 三步: 跑「改前」(上次保存的参数) → 跑「改后」(表单里现在这些) → 让 AI 读两份**实测**指标。
  // 前两步复用 `/api/backtest` 的既有作业(带 busy / 进度 / 结果保留那一整套), 这里只负责
  // 串起来 + 如实展示进度与失败原因。真正跑回测的代码在 strategy_backtest.js。

  /// 当前模式是否该显示评估面板: 参数模式 + 本页已经有改动。
  /// **没有改动就不显示** —— 摆一个点了只会 400 的按钮, 比不摆还糟。
  function syncAssessPanel() {
    const box = $("sg-ai-assess");
    if (!box) return;
    const turns = st.aiThreads.params;
    const last = turns.length ? turns[turns.length - 1] : null;
    const changed = last && last.changed ? last.changed : [];
    if (st.aiMode !== "params" || !changed.length || !st.detailCtx) {
      box.hidden = true;
      box.innerHTML = "";
      return;
    }
    box.hidden = false;
    box.innerHTML = "";
    box.appendChild(h("div", "sg-guide-title", t("sgAiAssessTitle")));
    box.appendChild(h("div", "sg-guide-body", t("sgAiAssessBody")));
    const actions = h("div", "sg-guide-actions");
    const btn = h("button", "sg-btn sg-btn-mini sg-btn-primary", t("sgAiAssessRun"));
    btn.type = "button";
    btn.id = "sg-ai-assess-go";
    btn.addEventListener("click", onAssessParams);
    actions.appendChild(btn);
    box.appendChild(actions);
    const msg = h("div", "sg-msg");
    msg.id = "sg-ai-assess-msg";
    msg.setAttribute("aria-live", "polite");
    box.appendChild(msg);
    box.appendChild(h("div", "sg-hint", t("sgAiAssessWindow")));
    // 结果区: 由 onAssessParams 往里写。**每次重建面板就是一次清空** —— 面板只在"改动集合变了"
    // 时重建, 所以清掉的那份结论本来就是关于**上一组**参数的, 留着比清掉更糟(会被当成本次的)。
    const result = h("div", "sg-ai-assess-result");
    result.id = "sg-ai-assess-result";
    box.appendChild(result);
  }

  /// 「跑回测评估影响」: 现场取两套参数 → 串行两次回测 → 问 AI。
  async function onAssessParams() {
    if (!st.detailCtx) return;
    const g = st.gen;
    const id = st.detailCtx.id;
    const msg = $("sg-ai-assess-msg");
    const pair = ($("sg-pair").value || "").trim();
    if (!pair) {
      setMsg(msg, t("sgPairRequired"), "err");
      return;
    }
    let newParams;
    try {
      newParams = gatherParams(false);
    } catch (err) {
      setMsg(msg, err.message, "err");
      return;
    }
    // 改前 = **上次保存的参数**(进详情页时读到的实例值): AI 改动是草稿, 还没落盘,
    // 所以这一份就是"当前在跑的那组"。改后 = 表单里现在这些(用户可能还手动调过)。
    const baseParams = Object.assign({}, (st.detailCtx.detail && st.detailCtx.detail.current) || {});
    const turns = st.aiThreads.params;
    const last = turns.length ? turns[turns.length - 1] : null;
    const changed = last && last.changed ? last.changed : [];
    const btn = $("sg-ai-assess-go");
    if (btn) btn.disabled = true;
    setMsg(msg, t("sgAiAssessRunningBase"), "");
    try {
      const out = await runParamAssess({
        id: id,
        pair: pair,
        baseParams: baseParams,
        newParams: newParams,
        changed: changed,
        g: g,
        // 进度文案在**本卡片**里更新: 别去动回测卡的 msg 区, 那是另一个动作的地盘。
        onStage: (stage) => {
          if (g !== st.gen) return;
          if (stage === "base") setMsg(msg, t("sgAiAssessRunningBase"), "");
          else if (stage === "new") setMsg(msg, t("sgAiAssessRunningNew"), "");
          else setMsg(msg, t("sgAiAssessAssessing"), "");
        },
      });
      if (g !== st.gen) return;
      setMsg(msg, "", "");
      const result = $("sg-ai-assess-result");
      if (result) {
        const block = h("div", "sg-ai-assess-block");
        block.appendChild(h("div", "sg-guide-body", out.advice));
        result.appendChild(block);
      }
    } catch (err) {
      if (g !== st.gen) return;
      setMsg(msg, t("sgAiAssessFailed") + ((err && err.message) || String(err)), "err");
    } finally {
      if (g === st.gen && btn) btn.disabled = false;
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

  /// 按**当前语言 + 当前模式**重刷模式相关文案(「执行」按钮 + 一行提示 + 输入框占位)。
  ///
  /// 同时写 `textContent` / `placeholder` 与 `data-zh` / `data-en`(-placeholder): 切语言时
  /// `R.applyLang` 会先按 data 属性把静态标签就地换一轮, 若只改可见值, 换语言就会把当前模式的
  /// 文案盖回**初始模式**那一份(N3 起占位文字也分模式, 这个坑从按钮扩大到了输入框)。
  function applyAiModeText() {
    const isParams = st.aiMode !== "code";
    const goKey = isParams ? "sgAiGoParams" : "sgAiGoCode";
    const hintKey = isParams ? "sgAiModeHintParams" : "sgAiModeHintCode";
    for (const [el, key] of [
      [$("sg-ai-go"), goKey],
      [$("sg-ai-mode-hint"), hintKey],
    ]) {
      el.textContent = R.TEXT[R.lang][key];
      el.dataset.zh = R.TEXT.zh[key];
      el.dataset.en = R.TEXT.en[key];
    }
    const phKey = isParams ? "sgAiPhParams" : "sgAiPhCode";
    const ph = $("sg-ai-instr");
    ph.placeholder = R.TEXT[R.lang][phKey];
    ph.dataset.zhPlaceholder = R.TEXT.zh[phKey];
    ph.dataset.enPlaceholder = R.TEXT.en[phKey];
    $("sg-ai-mode-params").classList.toggle("on", isParams);
    $("sg-ai-mode-code").classList.toggle("on", !isParams);
  }

  /// 切模式(点 chip / 详情挂载时初始化)。顺手清掉上一条行内消息 ——
  /// 那是**上一个模式**的结论, 留着会让人以为本次也成功了。
  /// N3: 线程按模式分开(`st.aiThreads`), 所以切模式是"换一条对话", 不是"丢历史"。
  function setAiMode(mode) {
    st.aiMode = mode === "code" ? "code" : "params";
    applyAiModeText();
    setMsg($("sg-ai-msg"), "", "");
    renderThread();
    syncAssessPanel();
  }

  function syncAiBtn(busy) {
    const btn = $("sg-ai-go");
    btn.disabled = !!busy;
    // 闲时按模式回写标签(切语言后 `refreshDynamic` → 这里, 也是它把文案换成新语言)。
    if (busy) btn.textContent = t("sgAiBusy");
    else applyAiModeText();
  }

  /// params 模式回包 → 填进参数表单。返回**真正填上**的键数。
  ///
  /// 只认表单里画得出来的行(清单声明的参数都在 `st.paramInputs` 里); `pair` 是独立字段
  /// (`#sg-pair`), 单独处理。服务端已把清单外的键过滤掉了, 这里再兜一层:
  /// 画不出来的键一律**不填**, 由调用方按返回的条数决定提示语 —— 绝不假报"已填好"。
  function fillParamInputs(suggested) {
    let n = 0;
    for (const p of st.paramInputs) {
      if (!Object.prototype.hasOwnProperty.call(suggested, p.key)) continue;
      const v = suggested[p.key];
      if (p.ty === "bool") p.input.checked = !!v;
      else p.input.value = v === null || v === undefined ? "" : String(v);
      n += 1;
    }
    const pair = suggested.pair;
    if (typeof pair === "string" && pair.trim()) {
      $("sg-pair").value = pair.trim();
      n += 1;
    }
    return n;
  }

  // ---------- 对外(外壳与其它子文件调用) ----------
  S.updateDirty = updateDirty;
  S.refreshHighlight = refreshHighlight;
  S.syncSaveBtn = syncSaveBtn;
  S.syncAiBtn = syncAiBtn;
  S.setAiMode = setAiMode;
  S.onCodeKeydown = onCodeKeydown;
  S.onSave = onSave;
  S.onAiEdit = onAiEdit;
  // N3: 外壳 mount 直接挂这两个(清空对话按钮 / 切模式后重建线程与评估面板)。
  S.onAiClearThread = onAiClearThread;
  S.renderAiThread = renderThread;
  S.syncAssessPanel = syncAssessPanel;
  // `onUndo` 也是外壳 `mount()` 直接挂到按钮上的回调(`$("sg-undo").addEventListener`),
  // 漏了这条导出 = 点开策略页整片 `ReferenceError: onUndo is not defined`(2026-10-07 真机走查抓到)。
  S.onUndo = onUndo;
})();
