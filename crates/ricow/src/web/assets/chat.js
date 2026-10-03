// Ricow Web 前端「对话」视图 (032): 032 从原 app.js 整体迁入, **逻辑零改写** ——
// 会话列表 / SSE onFrame / 输入 composer / 菜单 / 术语浮层 / 回测报告结构化 /
// 三个只读面板(交易 / 日志 / 策略)。公共件走 `window.Ricow`(common.js), 本模块只留
// 对话视图自己的状态与渲染, 并注册为 `R.views.chat`。
//
// 与 CLI 同源: 本文件**不做任何业务判断**。斜杠命令、菜单内容、轮次起止全由宿主决定
// (`crate::ai::session`), 这里只做三件事 —— 把 SSE 帧画出来、把输入区的一行 POST 回去、
// 按帧自带的级别着色。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  const els = {
    list: document.getElementById("session-list"),
    newBtn: document.getElementById("new-session"),
    langBtn: document.getElementById("lang-toggle"),
    title: document.getElementById("session-title"),
    status: document.getElementById("status"),
    stream: document.getElementById("stream"),
    form: document.getElementById("composer"),
    input: document.getElementById("input"),
    send: document.getElementById("send"),
    tradeAsof: document.getElementById("trade-asof"),
    tradeSource: document.getElementById("trade-source"),
    positions: document.getElementById("positions"),
    orders: document.getElementById("orders"),
    fills: document.getElementById("fills"),
    logName: document.getElementById("log-name"),
    logLines: document.getElementById("log-lines"),
    strategyList: document.getElementById("strategy-list"),
    strategyDetail: document.getElementById("strategy-detail"),
  };

  // 对话视图运行时状态(当前界面语言是全局唯一一份 `R.lang`, 不在此另存)。
  const state = {
    rows: [],
    sessionId: null,
    source: null,
    busy: false,
    secret: false,
    streaming: null,
    trade: null, // 交易面板最近一次读到的快照(还没读到就是 null, 不冒充"无持仓")
    logName: null, // 日志面板正在跟的策略
    logStream: null, // 日志面板那条流, 与会话流 state.source 各走各的(R5)
    strategyList: [], // 策略面板最近一次读到的列表(切语言重渲染用, 不重取)
    strategyDetail: null, // 策略面板正在展示详情的清单(null = 未点开)
    langCmd: false, // 本轮发过 /lang 命令: turn_end 后从服务端重读一次语言(D12, 对话内切换全视图生效)
  };

  // 术语表(FR-024 / FR-026): 服务端一次性给全, 前端只按当前语言挑一份 —— 切语言不重取。
  const terms = new Map();
  let termRe = null;
  let termKey = null;

  /// 对话视图的语言切换: 先跑通用部分(common.js: html lang / 语言钮 / data-zh 遍历),
  /// 再把本视图持有的动态内容按新语言重渲染。
  function applyLang(lang) {
    R.applyLang(lang);
    renderList();
    renderTrade();
    // 策略面板(031): 内容静态不轮询, 但切语言要跟着换分组标题/必填/适合文案 —— 用缓存重渲染, 不重取。
    if (els.strategyList) {
      renderStrategyList(state.strategyList);
      if (state.strategyDetail) renderStrategyDetail(state.strategyDetail);
    }
    // 日志**内容**不重译(那是文件里的原样行, FR-018); 只有宿主自己写的提示语要跟着切。
    for (const node of els.logLines.querySelectorAll("[data-note]")) {
      node.textContent = noteText(node.dataset.note, node.dataset.reason);
    }
    if (!pop.hidden) renderPop(); // 浮层开着就地换语言, 不必关掉重开
  }

  /// 回复期间禁用输入区并给出"正在思考"态(FR-009); 解除的唯一信号是 `turn_end` 帧。
  function setBusy(busy) {
    state.busy = busy;
    els.input.disabled = busy;
    els.send.disabled = busy;
    els.status.textContent = busy ? t("thinking") : "";
    if (!busy) els.input.focus();
  }

  function autosize() {
    els.input.style.height = "auto";
    els.input.style.height = Math.min(els.input.scrollHeight, 180) + "px";
  }

  // ---------- 对话流渲染 ----------

  /// 用户手动上滚时不再强制拉回(FR-008): 追加前先看是否贴着底。
  function atBottom() {
    const el = els.stream;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 60;
  }

  function append(node, pinned) {
    foldPrevious(); // 034: 新块进门前, 把上一块收成两行 —— 最新的一块始终完整可见
    els.stream.appendChild(node);
    if (pinned) els.stream.scrollTop = els.stream.scrollHeight;
  }

  // ---------- 历史消息两行折叠 (034) ----------
  //
  // 规则: 流里除最新一块外, 更早的 .msg / .line 默认收成两行, 点击展开(再点收起)。
  // `.menu`(宿主菜单)不参与 —— 它是交互块, 折起来就没法点了; 正在流式的气泡也不折。

  /// 只有这两种块参与折叠: 它们才可能长过两行。菜单 / 浮层等不在其列。
  function foldableBlock(node) {
    return node &&
        node.classList &&
        (node.classList.contains("msg") || node.classList.contains("line"))
      ? node
      : null;
  }

  /// 把一块收成两行; 内容本就不超两行的块撤掉标记 —— 没东西可展开的块不该装作可点。
  function foldUp(node) {
    if (!foldableBlock(node) || node === state.streaming || node.dataset.keepOpen) return;
    node.classList.add("foldable", "folded");
    node.dataset.foldHint = R.lang === "en" ? "··· click to expand" : "··· 点击展开";
    if (node.scrollHeight <= node.clientHeight + 1) {
      node.classList.remove("foldable", "folded");
      delete node.dataset.foldHint;
    }
  }

  /// 收起流里当前最后一块(在追加新块之前调用)。
  function foldPrevious() {
    foldUp(els.stream.lastElementChild);
  }

  function appendLine(sev, text) {
    const pinned = atBottom();
    if (sev === "notice") {
      const menu = parseMenu(text);
      if (menu) {
        append(renderMenu(menu), pinned);
        return;
      }
    }
    const div = document.createElement("div");
    div.className = "line " + (sev || "normal");
    div.textContent = text;
    append(div, pinned);
    renderRich(div); // 报告行结构化 + 术语可点
  }

  function appendMessage(role, content, sev) {
    if (role === "host") {
      appendLine(sev || "normal", content);
      return;
    }
    const div = document.createElement("div");
    div.className = "msg " + (role === "user" ? "user" : "assistant");
    div.textContent = content;
    append(div, atBottom());
    if (role !== "user") renderRich(div);
  }

  /// 助手文本增量 → 追加到当前回复气泡(FR-008 的"流式"), 没有气泡就开一个。
  function appendDelta(text) {
    const pinned = atBottom();
    if (!state.streaming) {
      foldPrevious(); // 034: 新回复气泡开张, 先把上一块收起来(走 append 的块由 append 收)
      state.streaming = document.createElement("div");
      state.streaming.className = "msg assistant";
      els.stream.appendChild(state.streaming);
    }
    state.streaming.textContent += text;
    if (pinned) els.stream.scrollTop = els.stream.scrollHeight;
  }

  /// 收起当前增量气泡: 此刻全文已到齐, 才做报告结构化与术语标注(流式期间改不了 DOM 结构)。
  function finishStreaming() {
    if (!state.streaming) return;
    renderRich(state.streaming);
    state.streaming = null;
  }

  /// 宿主菜单是 `menu::render` 的纯文本(标题 + 每行 "  序号. 标签", 经 `notice` 级别出来):
  /// 就地解析成可点击列表, 点击等价于输入对应序号(FR-010) —— 会话侧零改动(D16)。
  function parseMenu(text) {
    const lines = text.split("\n");
    if (lines.length < 2 || !lines[0].trim()) return null;
    const items = [];
    for (const raw of lines.slice(1)) {
      const matched = /^\s+(\d+)\.\s+(.+)$/.exec(raw);
      if (!matched) return null;
      items.push({ n: matched[1], label: matched[2] });
    }
    return items.length ? { title: lines[0], items: items } : null;
  }

  function renderMenu(menu) {
    const box = document.createElement("div");
    box.className = "menu";
    const title = document.createElement("div");
    title.className = "menu-title";
    title.textContent = menu.title;
    box.appendChild(title);
    for (const item of menu.items) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "menu-item";
      btn.textContent = item.n + ". " + item.label;
      btn.addEventListener("click", () => submitText(item.n));
      box.appendChild(btn);
    }
    return box;
  }

  // ---------- 术语与回测报告 (FR-024 / FR-025 / FR-026) ----------

  const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

  /// 取一次术语表并建索引: `/api/terms` 已把中英两份一起给全, 切换语言无需重取(FR-026)。
  async function loadTerms() {
    const keys = [];
    for (const term of await R.api("/api/terms")) {
      terms.set(term.key, term);
      keys.push(term.key);
    }
    // 长名优先: `手续费占比` 不该被 `手续费` 从中间截断。
    keys.sort((a, b) => b.length - a.length);
    termRe = keys.length ? new RegExp(keys.map(escapeRe).join("|"), "g") : null;
  }

  /// 报告段标题(`--- 风险/收益指标 (无风险利率 rf = 3.80%/年) ---`) → 标题节点; 其余行 null。
  function sectionTitle(line) {
    const matched = /^\s*---\s*(.+?)\s*---$/.exec(line);
    if (!matched) return null;
    const div = document.createElement("div");
    div.className = "report-section";
    div.textContent = matched[1];
    return div;
  }

  /// `指标名: 值` 且指标名在术语表里 → 结构化的一行; 其余行 null。
  ///
  /// 判据就是术语表的 key(与 `format_backtest_report` 逐字同源, D13), 所以只有**报告真有的**
  /// 指标会被结构化, 普通带冒号的日志行不受影响; 括注(`年化收益率 (几何)`)只作展示, 不参与匹配。
  function metricRow(line) {
    const matched = /^\s*(\S[^:]*?): (.+)$/.exec(line);
    if (!matched) return null;
    const label = matched[1].replace(/\s*\(.*$/, "").trim();
    if (!terms.has(label)) return null;
    const row = document.createElement("div");
    row.className = "report-row";
    const name = document.createElement("span");
    name.className = "name";
    name.appendChild(termButton(label));
    if (matched[1].length > label.length) {
      name.appendChild(document.createTextNode(matched[1].slice(label.length)));
    }
    const value = document.createElement("span");
    value.className = "value";
    value.textContent = matched[2];
    row.appendChild(name);
    row.appendChild(value);
    return row;
  }

  /// 把一段输出里的**回测报告**结构化(FR-025), 并给术语挂上可点击入口(FR-024)。
  ///
  /// 只动报告自己: 连续的 `指标名: 值` / `--- 段标题 ---` 收进一张卡片, 其余正文一字不动 ——
  /// 哪些名字算指标全部来自术语表, 前端不另立第二套定义。
  function renderRich(el) {
    const text = el.textContent || "";
    if (text.indexOf(":") >= 0) {
      const frag = document.createDocumentFragment();
      let plain = [];
      let card = null;
      const flushPlain = () => {
        if (!plain.length) return;
        frag.appendChild(document.createTextNode(plain.join("\n")));
        plain = [];
      };
      const flushCard = () => {
        if (!card) return;
        frag.appendChild(card);
        card = null;
      };
      for (const line of text.split("\n")) {
        const node = sectionTitle(line) || metricRow(line);
        if (node) {
          flushPlain();
          if (!card) {
            card = document.createElement("div");
            card.className = "report";
          }
          card.appendChild(node);
          continue;
        }
        flushCard();
        plain.push(line);
      }
      flushPlain();
      flushCard();
      el.textContent = "";
      el.appendChild(frag);
    }
    decorateTerms(el);
  }

  /// 把正文里出现的术语包成可点击入口(FR-024); 已包好的不再进, 避免嵌套。
  function decorateTerms(root) {
    if (!termRe) return;
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    for (const node of nodes) {
      if (node.parentElement && node.parentElement.classList.contains("term")) continue;
      const text = node.nodeValue;
      termRe.lastIndex = 0;
      let matched = termRe.exec(text);
      if (!matched) continue;
      const frag = document.createDocumentFragment();
      let last = 0;
      while (matched) {
        if (matched.index > last) {
          frag.appendChild(document.createTextNode(text.slice(last, matched.index)));
        }
        frag.appendChild(termButton(matched[0]));
        last = matched.index + matched[0].length;
        matched = termRe.exec(text);
      }
      if (last < text.length) frag.appendChild(document.createTextNode(text.slice(last)));
      node.parentNode.replaceChild(frag, node);
    }
  }

  function termButton(key) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "term";
    btn.textContent = key;
    btn.title = t("termHint");
    btn.addEventListener("click", (ev) => {
      ev.stopPropagation(); // 点术语是"看解释", 不该顺带触发展开的会话项
      openTerm(key, btn);
    });
    return btn;
  }

  /// 解释浮层: 单例, 点术语开、点别处 / Esc 关。正文按**当前语言**现取(FR-024 / FR-028)。
  const pop = document.createElement("div");
  pop.className = "term-pop";
  pop.hidden = true;
  document.body.appendChild(pop);

  function openTerm(key, anchor) {
    if (!terms.has(key)) return;
    termKey = key;
    renderPop();
    pop.hidden = false;
    placePop(anchor);
  }

  function closeTerm() {
    if (pop.hidden) return;
    pop.hidden = true;
    termKey = null;
  }

  function renderPop() {
    const term = terms.get(termKey);
    if (!term) return;
    pop.textContent = "";
    const name = document.createElement("div");
    name.className = "term-name";
    name.textContent = term.key;
    const body = document.createElement("div");
    body.className = "term-body";
    body.textContent = R.lang === "en" ? term.en : term.zh;
    const hint = document.createElement("div");
    hint.className = "term-hint";
    hint.textContent = t("termClose");
    pop.appendChild(name);
    pop.appendChild(body);
    pop.appendChild(hint);
  }

  /// 贴在术语下方, 下方放不下就翻到上方, 并夹在视口内。
  function placePop(anchor) {
    const rect = anchor.getBoundingClientRect();
    let left = Math.min(rect.left, window.innerWidth - pop.offsetWidth - 8);
    left = Math.max(8, left);
    let top = rect.bottom + 6;
    if (top + pop.offsetHeight > window.innerHeight - 8) {
      top = Math.max(8, rect.top - pop.offsetHeight - 6);
    }
    pop.style.left = left + "px";
    pop.style.top = top + "px";
  }

  // ---------- SSE ----------

  function connect(id) {
    if (state.source) state.source.close();
    const url =
      "/api/sessions/" + encodeURIComponent(id) + "/events?token=" + encodeURIComponent(R.TOKEN);
    const handle = R.sse(url, {
      onmessage: (ev) => {
        if (state.source !== handle) return; // 切会话后旧连接可能还有余帧, 丢
        let frame;
        try {
          frame = JSON.parse(ev.data);
        } catch (_) {
          return;
        }
        onFrame(frame, handle);
      },
      // 放弃重连时不另起提示: 服务真停了, 下一次发送失败会如实告诉用户(与既往一致)。
    });
    state.source = handle;
  }

  function onFrame(frame, es) {
    switch (frame.type) {
      case "delta":
        appendDelta(frame.text);
        break;
      case "line":
        finishStreaming(); // 整行输出 = 本轮增量结束
        appendLine(frame.sev, frame.text);
        break;
      case "secret_prompt":
        // 会话要读一次不回显的密钥(FR-012): 切遮蔽输入, 明文不进对话流(SC-011)。
        // 输入区是 `<textarea>`, 它的 `type` 是**只读属性**(赋值直接抛 TypeError, 见 `setMasked`)。
        state.secret = true;
        setMasked(true);
        els.input.placeholder = frame.prompt;
        els.input.focus();
        break;
      case "turn_end":
        // 上一轮收尾(唯一分界线): 解除禁用, 恢复普通输入。
        finishStreaming();
        state.secret = false;
        setMasked(false);
        applyLang(R.lang);
        setBusy(false);
        // 这一轮可能刚下过单 / 刚启动过策略: 两侧面板都重新取一次(读不到就如实说)。
        refreshTrades().catch(tradeError);
        refreshLogList().catch(() => {});
        // 本轮若发过 /lang: 服务端已改 `[ui].lang`(D12), 从服务端重读并全视图生效。
        if (state.langCmd) {
          state.langCmd = false;
          R.api("/api/lang")
            .then((info) => {
              if (info && info.lang && info.lang !== R.lang) applyLang(info.lang);
            })
            .catch(() => {});
        }
        break;
      case "closed":
        finishStreaming();
        if (state.source === es) {
          state.source.close();
          state.source = null;
        }
        setBusy(false);
        appendLine("warn", t("ended"));
        break;
    }
  }

  // ---------- 会话 ----------

  function renderList() {
    els.list.textContent = "";
    for (const row of state.rows) {
      const li = document.createElement("li");
      if (row.id === state.sessionId) li.className = "active";
      const head = document.createElement("div");
      head.className = "row";
      const title = document.createElement("span");
      title.className = "title";
      title.textContent = row.title || t("untitled");
      const del = document.createElement("button");
      del.type = "button";
      del.className = "del";
      del.textContent = "×";
      del.title = t("delete");
      del.addEventListener("click", (ev) => {
        ev.stopPropagation(); // 删除不是"打开会话", 别把点击顺带给列表项
        deleteSession(row.id).catch((err) => appendLine("error", t("failed") + err.message));
      });
      head.appendChild(title);
      head.appendChild(del);
      const time = document.createElement("span");
      time.className = "time";
      time.textContent = R.formatTime(row.updated_at);
      li.appendChild(head);
      li.appendChild(time);
      li.addEventListener("click", () => openSession(row.id));
      els.list.appendChild(li);
    }
  }

  async function refreshSessions() {
    state.rows = await R.api("/api/sessions");
    // 新会话没有标题(留给首条用户消息生成, FR-022), 列表里显示占位。
    for (const row of state.rows) {
      if (!row.title) row.title = t("newSession");
    }
    renderList();
  }

  /// 打开一个会话: 先回放已落库的消息(FR-021), 再连帧流 —— 顺序反了会把欢迎语插在历史中间。
  async function openSession(id) {
    state.sessionId = id;
    state.secret = false;
    state.streaming = null;
    closeTerm();
    els.stream.textContent = "";
    setMasked(false);
    els.title.textContent = "";
    state.rows.forEach((row) => {
      if (row.id === id) els.title.textContent = row.title || t("untitled");
    });
    renderList();
    if (state.source) {
      state.source.close();
      state.source = null;
    }
    setBusy(false);
    try {
      const messages = await R.api("/api/sessions/" + encodeURIComponent(id) + "/messages");
      for (const m of messages) appendMessage(m.role, m.content, m.sev);
    } catch (_) {
      // 空会话 / 读取失败: 对话流保持空白即可, 帧流会补上新的内容。
    }
    els.stream.scrollTop = els.stream.scrollHeight;
    connect(id);
  }

  async function createSession() {
    const created = await R.api("/api/sessions", { method: "POST" });
    await refreshSessions();
    await openSession(created.id);
  }

  /// 删除会话(FR-007): 二次确认后才发 DELETE。
  ///
  /// 删的若是当前会话, 它的线程与消息都已作废(级联删除), 必须换一个落脚点 —— 否则输入区会继续
  /// 往一个不存在的会话里发; 列表空了就新建一个, 保证页面上永远有一个可用会话。
  async function deleteSession(id) {
    if (!window.confirm(t("confirmDelete"))) return;
    await R.api("/api/sessions/" + encodeURIComponent(id), { method: "DELETE" });
    const wasCurrent = id === state.sessionId;
    if (wasCurrent) {
      state.sessionId = null;
      if (state.source) {
        state.source.close();
        state.source = null;
      }
      els.stream.textContent = "";
      els.title.textContent = "";
    }
    await refreshSessions();
    if (!wasCurrent) return;
    if (state.rows.length) await openSession(state.rows[0].id);
    else await createSession();
  }

  // ---------- 输入 ----------

  /// 切密钥输入的回显遮蔽(FR-012 / SC-011)。
  ///
  /// 输入区是 `<textarea>`(要支持多行), 而它的 `type` 是**只读属性** —— 赋值会抛
  /// `TypeError: Cannot set property type of #<HTMLTextAreaElement> which has only a getter`,
  /// 在 `"use strict"` 下直接中断调用方, 所以遮蔽只能走 CSS(`#input.masked`, 见 `style.css`)。
  function setMasked(on) {
    els.input.classList.toggle("masked", on);
  }

  async function submitText(text) {
    const value = text === undefined ? els.input.value : text;
    // 空输入不发送(FR-009)。**密钥期例外**: 提示语写的是"回车放弃", 空值就是放弃信号,
    // 服务端也按"输入为空, 未改动任何配置"处理(`read_secret`)—— 在这里挡掉会让用户没法退出录入。
    if (state.busy || !state.sessionId || (!value.trim() && !state.secret)) return;
    const wasSecret = state.secret;
    state.secret = false;
    state.langCmd = /^\/lang(?:\s|$)/.test(value.trim()); // /lang 命令轮: turn_end 后重同步语言
    if (wasSecret) {
      setMasked(false);
      applyLang(R.lang);
    } else {
      appendMessage("user", value, "normal"); // 明文密钥不进对话流(SC-011)
    }
    els.input.value = "";
    autosize();
    setBusy(true);
    try {
      const res = await R.api(
        "/api/sessions/" + encodeURIComponent(state.sessionId) + "/input",
        { method: "POST", body: { text: value } },
      );
      if (!res.accepted) {
        setBusy(false);
        appendLine("warn", t("ended"));
      }
    } catch (err) {
      setBusy(false);
      appendLine("error", t("failed") + err.message);
    }
  }

  els.input.addEventListener("keydown", (ev) => {
    // Enter 发送 / Shift+Enter 换行(FR-009); 中文输入法组字期间的回车不算。
    if (ev.key === "Enter" && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      submitText();
    }
  });
  els.form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    submitText();
  });
  els.input.addEventListener("input", autosize);

  els.newBtn.addEventListener("click", () => {
    createSession().catch((err) => appendLine("error", t("failed") + err.message));
  });

  els.langBtn.addEventListener("click", async () => {
    const next = R.lang === "zh" ? "en" : "zh";
    try {
      const info = await R.api("/api/lang", { method: "POST", body: { lang: next } });
      applyLang(info.lang);
    } catch (err) {
      appendLine("error", t("failed") + err.message);
    }
  });

  // 主题切换(034): change 即写盘 —— 值存 `ricow.toml` 的 `[ui].theme`, 与语言同一来源;
  // 失败回滚选项并如实报错。R.applyTheme 由 common.js 提供, 切换会广播 `ricow:theme`。
  document.getElementById("theme-select").addEventListener("change", async (ev) => {
    const wanted = ev.target.value;
    try {
      const info = await R.api("/api/theme", { method: "POST", body: { theme: wanted } });
      R.applyTheme(info.theme);
    } catch (err) {
      ev.target.value = R.theme;
      appendLine("error", t("failed") + err.message);
    }
  });

  // 浮层的关闭手势(FR-024): 点别处 / Esc; 滚动或改窗宽后位置不再贴合, 一并收起。
  document.addEventListener("click", (ev) => {
    if (pop.contains(ev.target)) return; // 浮层内的点击(选中文字)不该关掉它
    closeTerm();
  });
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") closeTerm();
  });
  els.stream.addEventListener("scroll", closeTerm, { passive: true });
  window.addEventListener("resize", closeTerm);

  // 历史消息折叠(034): 点收起的块展开、再点收起。
  // 例外都不抢手势: 点术语(顺带把它所在的折叠块展开, 否则解释会被截断看不见)、
  // 点任何按钮 / 链接 / 菜单、以及正在选文字(复制)时。
  els.stream.addEventListener("click", (ev) => {
    const target = ev.target;
    if (target.closest(".menu, button, a")) return;
    const term = target.closest(".term");
    if (term) {
      const owner = term.closest(".foldable");
      if (owner && owner.classList.contains("folded")) {
        owner.classList.remove("folded");
        owner.dataset.keepOpen = "1";
      }
      return;
    }
    const selection = window.getSelection();
    if (selection && String(selection).length) return;
    const block = target.closest(".foldable");
    if (!block || !els.stream.contains(block)) return;
    if (block.classList.contains("folded")) {
      block.classList.remove("folded");
      block.dataset.keepOpen = "1"; // 手动展开过的, 新消息到达时不再被自动收起
    } else {
      block.classList.add("folded");
      delete block.dataset.keepOpen;
    }
  });

  // ---------- 交易面板 (026 FR-012 / FR-013 / FR-014; D11 / D17) ----------
  //
  // 只读: 数据全部来自 `/api/trades/*`, 面板里**没有**任何直连交易所的按钮 —— 撤单 / 平仓 /
  // 停机仍然只能在中间那栏走对话确认(D17 / FR-013)。空状态三态**分开**呈现(FR-014)。

  /// 三态的严重度: 任一路读不到, 就不许把这块面板读成"确实没有"。
  const SOURCE_RANK = { ok: 0, daemon_down: 1, unreadable: 2 };
  const rank = (source) => SOURCE_RANK[source] || 0;

  /// 刷新节奏: 用户可能刚在对话里下了单, 策略也可能正在后台跑 —— "当前持仓"得跟着动。
  const TRADE_POLL_MS = 5000;

  function emptyRow(text) {
    const div = document.createElement("div");
    div.className = "empty";
    div.textContent = text;
    return div;
  }

  /// 时间戳 → 展示串。交易端点的 `updated_at` / `timestamp` 一律是**毫秒** —— 与会话列表的
  /// 秒级 `updated_at` 不同口径, 不能复用 `R.formatTime`。
  function formatStamp(ms) {
    if (!ms) return "";
    const locale = R.lang === "en" ? "en-US" : "zh-CN";
    return new Date(ms).toLocaleString(locale, {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }

  /// 一行小卡: `[[名, 值, 类], ...]` —— 名值分栏, 值用等宽(金额 / 数量对得齐)。
  function tradeRow(cells) {
    const row = document.createElement("div");
    row.className = "trow";
    for (const [key, value, cls] of cells) {
      const pair = document.createElement("div");
      pair.className = "pair";
      const name = document.createElement("span");
      name.className = "k";
      name.textContent = key;
      const val = document.createElement("span");
      val.className = cls ? "v " + cls : "v";
      val.textContent = value;
      pair.appendChild(name);
      pair.appendChild(val);
      row.appendChild(pair);
    }
    return row;
  }

  /// 每块的行位: 读不到时**绝不**写"无 X"(那正是假阴性), 只写"状态不可知"(FR-014)。
  function renderRows(box, nodes, emptyKey) {
    box.textContent = "";
    if (state.trade.source === "unreadable") {
      box.appendChild(emptyRow(t("unknown")));
      return;
    }
    if (!nodes.length) {
      box.appendChild(emptyRow(t(emptyKey)));
      return;
    }
    for (const node of nodes) box.appendChild(node);
  }

  function positionRow(p) {
    return tradeRow([
      [p.strategy_id + " · " + p.pair, p.size],
      [t("lEntry"), p.entry_price],
    ]);
  }

  function orderRow(o) {
    return tradeRow([
      [o.strategy_id + " · " + o.pair, o.side + " " + o.size],
      [t("lFilled"), o.filled_size + " @ " + o.price],
      [t("lStatus"), o.status + " · " + o.mode],
      [t("lOrderId"), o.exchange_order_id],
    ]);
  }

  function fillRow(f) {
    return tradeRow([
      [formatStamp(f.timestamp), f.side + " " + f.fill_size + " @ " + f.fill_price],
      // `mode` 为 null = 关联不到订单, **未知**(不猜, D5)。
      [
        f.strategy_id + " · " + f.pair,
        (f.mode || t("lUnknown")) + " · " + t("lFee") + " " + f.fee,
      ],
    ]);
  }

  /// 面板级三态 = 三路响应里最严重的一路(读不到 > daemon 未运行 > 正常)。
  function panelSource(replies) {
    let worst = { source: "ok" };
    for (const reply of replies) {
      if (reply && rank(reply.source) > rank(worst.source)) worst = reply;
    }
    return worst;
  }

  /// 三态文案: 正常不打扰; daemon 未运行 → 明说"此刻状态不可知"; 读不到 → 明说读不到 + 原因。
  /// 后两者都**不得**被读成"没有"(FR-014 / D10)。
  function sourceLine(view) {
    if (view.source === "unreadable") {
      return { cls: "unreadable", text: t("unreadable") + (view.reason ? ": " + view.reason : "") };
    }
    if (view.source === "daemon_down") return { cls: "daemon_down", text: t("daemonDown") };
    return { cls: "ok", text: "" };
  }

  function renderTrade() {
    if (!state.trade) return; // 还没读到过: 一片空白好过假称"无持仓"
    const view = state.trade;
    // 快照只按"最后写入时间"读, 不能说成"当前"(D11)。
    els.tradeAsof.textContent = view.updatedAt ? t("asof") + formatStamp(view.updatedAt) : "";
    const line = sourceLine(view);
    els.tradeSource.className = "source " + line.cls;
    els.tradeSource.textContent = line.text;
    els.tradeSource.hidden = !line.text;
    renderRows(els.positions, view.positions.map(positionRow), "emptyPositions");
    renderRows(els.orders, view.orders.map(orderRow), "emptyOrders");
    renderRows(els.fills, view.fills.map(fillRow), "emptyFills");
  }

  /// 拉一次三路只读数据(持仓 / 挂单 / 成交)。**只有 GET** —— 面板没有任何写动作(D17)。
  async function refreshTrades() {
    const [positions, orders, fills] = await Promise.all([
      R.api("/api/trades/positions"),
      R.api("/api/trades/orders"),
      R.api("/api/trades/fills"),
    ]);
    const source = panelSource([positions, orders, fills]);
    state.trade = {
      source: source.source,
      reason: source.reason || null,
      positions: positions.items || [],
      orders: orders.items || [],
      fills: fills.items || [],
      updatedAt: Math.max(
        positions.updated_at || 0,
        orders.updated_at || 0,
        fills.updated_at || 0,
      ),
    };
    renderTrade();
  }

  /// 连端点都够不着(服务异常 / 网络断了)也是"读不到": 如实说, 不许留着旧数字冒充"当前"。
  function tradeError(err) {
    state.trade = {
      source: "unreadable",
      reason: String((err && err.message) || err),
      positions: [],
      orders: [],
      fills: [],
      updatedAt: 0,
    };
    renderTrade();
  }

  // ---------- 日志面板 (026 FR-015 ~ FR-018; D2 / D12 / R5) ----------
  //
  // 与会话 SSE **两条独立的流**: 这里再开一个 `EventSource`, 不碰 `state.source` 那条会话流。

  /// 打开先给最近多少行; 服务端还会再夹一次硬上限(FR-015 / SC-008)。
  const LOG_TAIL_LINES = 200;
  /// DOM 里只留最近这么多行 —— 长时间开着也不会越堆越慢(文件本身零改动)。
  const LOG_DOM_MAX = 500;

  /// 宿主提示语的双语文案: `data-note` 存键名, `data-reason` 存实证原因, 切语言时就地重译。
  function noteText(key, reason) {
    return reason ? t(key) + ": " + reason : t(key);
  }

  /// 策略清单(FR-017): 只认 `logs/*.log` 文件 —— 策略**没在跑**, 只要有文件就列得出来。
  async function refreshLogList() {
    const files = await R.api("/api/logs");
    const keep = state.logName;
    els.logName.textContent = "";
    for (const file of files) {
      const option = document.createElement("option");
      option.value = file.name;
      option.textContent = file.name;
      els.logName.appendChild(option);
    }
    els.logName.disabled = !files.length;
    if (!files.length) {
      closeLog();
      els.logLines.textContent = "";
      els.logLines.appendChild(emptyRow(t("noLogs")));
      return;
    }
    // 选中的策略还在就**不重开**流(重开会清屏重读首屏, 白白打断正在跟的滚动位置)。
    els.logName.value = files.some((f) => f.name === keep) ? keep : files[0].name;
    if (els.logName.value !== state.logName) openLog(els.logName.value);
  }

  /// 打开某策略的日志流: 首屏(最近 N 行)由服务端在返回响应**之前**读好, 随后每 500ms 推增量
  /// (D12 / FR-016); 页面刷新重连即重建(FR-017)。
  function openLog(name) {
    closeLog();
    if (!name) return;
    state.logName = name;
    els.logLines.textContent = "";
    const url =
      "/api/logs/" +
      encodeURIComponent(name) +
      "/stream?lines=" +
      LOG_TAIL_LINES +
      "&token=" +
      encodeURIComponent(R.TOKEN);
    const handle = R.sse(url, {
      onmessage: (ev) => {
        if (state.logStream !== handle) return; // 切策略后旧流可能还有余帧, 丢
        let event;
        try {
          event = JSON.parse(ev.data);
        } catch (_) {
          return;
        }
        appendLog(event);
      },
    });
    state.logStream = handle;
  }

  function closeLog() {
    if (state.logStream) {
      state.logStream.close();
      state.logStream = null;
    }
    state.logName = null;
  }

  /// 日志帧落地(FR-018): `line` 是文件里的**原样**那一行(不着色、不解析、不改写);
  /// `rotated` / `unreadable` 是宿主的如实提示(D13), 用 `data-note` 记下来以便切语言时重译。
  function appendLog(event) {
    const div = document.createElement("div");
    if (event.kind === "line") {
      div.className = "log-line";
      div.textContent = event.text;
    } else {
      const key = event.kind === "unreadable" ? "logUnreadable" : "rotated";
      div.className = "log-note" + (event.kind === "unreadable" ? " unreadable" : "");
      div.dataset.note = key;
      if (event.kind === "unreadable") div.dataset.reason = event.text;
      div.textContent = noteText(key, div.dataset.reason);
    }
    els.logLines.appendChild(div);
    while (els.logLines.childElementCount > LOG_DOM_MAX) {
      els.logLines.removeChild(els.logLines.firstChild);
    }
    els.logLines.scrollTop = els.logLines.scrollHeight;
  }

  els.logName.addEventListener("change", () => {
    openLog(els.logName.value);
  });

  // ---------- 策略目录 (031 FR-013 / FR-014): 只读展示, 配置走对话确认 ----------

  /// 拉一次策略目录(内置示例 + 用户自写), 按现货/合约分组展示。
  async function loadStrategies() {
    const list = await R.api("/api/strategies");
    state.strategyList = list;
    renderStrategyList(list);
  }

  function renderStrategyList(list) {
    els.strategyList.textContent = "";
    if (!list.length) {
      els.strategyList.appendChild(emptyRow(t("noStrategies")));
      return;
    }
    for (const market of ["spot", "futures"]) {
      const group = list.filter((s) => s.market === market);
      if (!group.length) continue;
      const head = document.createElement("div");
      head.className = "strategy-group";
      head.textContent = market === "spot" ? t("spot") : t("futures");
      els.strategyList.appendChild(head);
      for (const s of group) {
        const item = document.createElement("div");
        item.className = "strategy-item";
        const name = document.createElement("span");
        name.className = "strategy-name";
        name.textContent = s.name;
        const summary = document.createElement("div");
        summary.className = "strategy-summary";
        summary.textContent = s.summary;
        item.appendChild(name);
        item.appendChild(summary);
        item.addEventListener("click", () => openStrategy(s.id).catch(strategyError));
        els.strategyList.appendChild(item);
      }
    }
  }

  /// 点开某策略 → 取完整清单(含参数 schema)渲染详情。
  async function openStrategy(id) {
    const detail = await R.api("/api/strategies/" + encodeURIComponent(id));
    state.strategyDetail = detail;
    renderStrategyDetail(detail);
  }

  function renderStrategyDetail(m) {
    const box = els.strategyDetail;
    box.textContent = "";
    box.hidden = false;
    const title = document.createElement("div");
    title.className = "strategy-title";
    title.textContent = m.name;
    box.appendChild(title);
    if (m.description) {
      const desc = document.createElement("div");
      desc.className = "strategy-desc";
      desc.textContent = m.description;
      box.appendChild(desc);
    }
    if (m.suitable || m.unsuitable) {
      const fit = document.createElement("div");
      fit.className = "strategy-fit";
      let text = "";
      if (m.suitable) text += t("suitable") + m.suitable;
      if (m.unsuitable) text += (text ? " · " : "") + t("unsuitable") + m.unsuitable;
      fit.textContent = text;
      box.appendChild(fit);
    }
    for (const p of m.params) box.appendChild(paramRow(p));
  }

  /// 一个参数的只读展示: 中文名(+ 必填)、说明、按类型预填默认值的控件。
  function paramRow(p) {
    const row = document.createElement("div");
    row.className = "param";
    const label = document.createElement("div");
    label.className = "param-label";
    label.textContent = p.name + (p.required ? " (" + t("required") + ")" : "");
    const desc = document.createElement("div");
    desc.className = "param-desc";
    desc.textContent = p.desc;
    row.appendChild(label);
    row.appendChild(desc);
    row.appendChild(paramInput(p));
    return row;
  }

  /// 控件一律 disabled(只读展示 + 默认值预填) —— 改参数走对话确认, 前端无写按钮(FR-016)。
  function paramInput(p) {
    const dflt = p.default;
    if (p.type === "enum") {
      const sel = document.createElement("select");
      sel.disabled = true;
      for (const o of p.options || []) {
        const opt = document.createElement("option");
        opt.value = o;
        opt.textContent = o;
        if (dflt !== undefined && dflt !== null && String(dflt) === o) opt.selected = true;
        sel.appendChild(opt);
      }
      return sel;
    }
    if (p.type === "bool") {
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.disabled = true;
      cb.checked = dflt === true;
      return cb;
    }
    const inp = document.createElement("input");
    inp.disabled = true;
    inp.type = p.type === "f64" || p.type === "i64" ? "number" : "text";
    if (dflt !== undefined && dflt !== null) inp.value = String(dflt);
    return inp;
  }

  function strategyError(err) {
    state.strategyDetail = null; // 别让切语言把旧详情盖回错误提示
    els.strategyDetail.hidden = false;
    els.strategyDetail.textContent = "";
    els.strategyDetail.appendChild(emptyRow(t("failed") + (err && err.message)));
  }

  // ---------- 启动 ----------

  async function boot() {
    try {
      applyLang((await R.api("/api/lang")).lang); // 与 CLI 共用 `[ui].lang`(D12)
    } catch (_) {
      applyLang("zh");
    }
    try {
      R.applyTheme((await R.api("/api/theme")).theme); // 034: 与 CLI 共用 `[ui].theme`
    } catch (_) {
      R.applyTheme("dark"); // 读不到(服务异常): 按深色兜底, 不影响其余功能
    }
    try {
      await loadTerms();
    } catch (_) {
      // 术语表拿不到(服务异常): 术语不可点、报告按原样显示, 其余功能不受影响。
    }
    try {
      await refreshSessions();
    } catch (err) {
      appendLine("error", t("failed") + err.message);
    }
    // 默认打开最近活动的会话; 一个都没有就先建一个。
    if (state.rows.length) await openSession(state.rows[0].id);
    else await createSession();
    // 右侧两块面板(026): 启动先各取一次, 之后交易每 5s 自动跟, 日志靠流推。
    refreshTrades().catch(tradeError);
    refreshLogList().catch((err) => appendLine("error", t("failed") + err.message));
    // 策略目录(031): 启动取一次, 内容静态(清单), 不轮询。
    loadStrategies().catch((err) => appendLine("error", t("failed") + err.message));
    window.setInterval(() => {
      refreshTrades().catch(tradeError);
    }, TRADE_POLL_MS);
  }

  /// 初始化只跑一次(视图反复激活 / 入口与路由都可能触发): 节拍与会话连接与拆分前完全一致。
  let bootPromise = null;
  function init() {
    if (!bootPromise) {
      bootPromise = boot().catch((err) => appendLine("error", t("failed") + err.message));
    }
    return bootPromise;
  }

  R.chat = { init: init };
  // 注册对话视图: 阶段 1 不提供 deactivate —— 切到别的视图时会话 SSE 与 5s 轮询照旧,
  // 与拆分前(页面只有一个视图)行为逐字一致。
  R.views.chat = {
    activate: () => {
      init();
    },
  };
})();
