// Ricow Web 前端「策略」视图 — 详情与参数表单 (042 从 strategies.js 拆出):
//   enterDetail(取详情+源码) / 徽章 / 诚实性提示 / 参数表单(参数行 + 保留键跳过) /
//   参数清单编辑器(P2-8)。
// 取用: 工具与共享状态来自 `R.sg`(由 strategies.js 建立); 跨文件符号见本文件末尾的 `S.xxx =`。
// 本文件**不做**: 列表态、新建/复制、Lua 编辑器、保存、回测 —— 各归其文件。
"use strict";

(function () {
  const R = window.Ricow;
  const S = R.sg;
  const t = S.t;
  const $ = S.$;
  const h = S.h;
  const setMsg = S.setMsg;
  const st = S.st;
  const DEFAULT_PAIR = S.DEFAULT_PAIR;
  const RESERVED_PARAM_KEYS = S.RESERVED_PARAM_KEYS;

  // 跨文件符号(042 D2: 延迟查找)
  const syncSaveBtn = (...a) => S.syncSaveBtn(...a);
  const syncAiBtn = (...a) => S.syncAiBtn(...a);
  const syncBtBtn = (...a) => S.syncBtBtn(...a);
  const refreshHighlight = (...a) => S.refreshHighlight(...a);
  const updateDirty = (...a) => S.updateDirty(...a);
  const startPolling = (...a) => S.startPolling(...a);
  const renderJob = (...a) => S.renderJob(...a);
  // 052: 引导卡的「问 AI 怎么配」要把 AI 卡切到调参模式 —— 走 editor 子文件的唯一入口。
  const setAiMode = (...a) => S.setAiMode(...a);
  // N3: 详情渲染后要重画对话线程与评估面板(它们依赖 detailCtx)。
  const renderAiThread = (...a) => S.renderAiThread(...a);
  const syncAssessPanel = (...a) => S.syncAssessPanel(...a);

  // ---------- 详情态 ----------

  async function enterDetail(id, g) {
    st.detailCtx = null;
    st.paramInputs = [];
    // 053: 交易对基线一并清零 —— 它与参数共用一套"改没改"判据。
    st.savedPair = "";
    st.savedCode = "";
    st.aiDraft = false;
    st.saveLocked = false;
    // N3: 对话线程**按策略清空** —— 它是"这个策略这组参数的讨论", 换个策略接着聊会把上一个
    // 策略的上下文带过去(历史会被原样拼进提示词)。两个模式各一条, 一并复位。
    st.aiThreads = { params: [], code: [] };
    $("sg-params").innerHTML = "";
    $("sg-code").value = "";
    $("sg-code").readOnly = false;
    $("sg-ai-instr").value = "";
    setMsg($("sg-savemsg"), "", "");
    setMsg($("sg-ai-msg"), "", "");
    setMsg($("sg-bt-msg"), "", "");
    $("sg-bt-result").innerHTML = "";
    // 参数扫描 / 多维寻优的结果是**按当前策略的参数**算出来的 —— 换策略必须一并清掉: 详情骨架
    // 只在 mount 时建一次, 不清就是上一份内容留在原地, A 策略的结果会挂到 B 名下(真机走查复现)。
    setMsg($("sg-sw-msg"), "", "");
    $("sg-sw-result").innerHTML = "";
    $("sg-sw-values").value = "";
    setMsg($("sg-opt-msg"), "", "");
    $("sg-opt-result").innerHTML = "";
    $("sg-opt-dims").innerHTML = ""; // 维度行随策略重建(键下拉取自该策略的参数清单)
    syncOptAddBtn(); // 行数清零后「添加维度」钮回到可用
    // 两个折叠框复位收起: 展开着的空框看着像"还在跑", 也让用户以为上一份结果还在。
    $("sg-sweep-box").open = false;
    $("sg-opt-box").open = false;
    $("sg-dirty").hidden = true;
    $("sg-builtin-hint").hidden = true;
    // 适用性卡片: 装载前先清掉上一份的残留, 免得点 A 看 B 时先闪一眼旧文案。
    $("sg-fit-card").hidden = true;
    $("sg-fit-body").innerHTML = "";
    syncSaveBtn("idle");
    syncAiBtn(false);
    syncBtBtn(false);

    const loadBox = $("sg-d-loadmsg");
    loadBox.innerHTML = "";
    loadBox.appendChild(h("div", "sg-loading", t("sgLoading")));
    $("sg-d-name").textContent = id;

    async function load() {
      const [detail, source] = await Promise.all([
        R.api("/api/strategies/" + encodeURIComponent(id)),
        R.api("/api/strategies/" + encodeURIComponent(id) + "/source"),
      ]);
      return { detail, source };
    }

    try {
      const { detail, source } = await load();
      if (g !== st.gen) return;
      loadBox.innerHTML = "";
      renderDetail(id, detail, source, g);
    } catch (err) {
      if (g !== st.gen) return;
      loadBox.innerHTML = "";
      const box = h("div", "sg-error-box");
      box.appendChild(h("div", "sg-msg err", (err && err.message) || String(err)));
      const btn = h("button", "sg-btn", t("sgRetry"));
      btn.type = "button";
      btn.addEventListener("click", () => enterDetail(id, g));
      box.appendChild(btn);
      loadBox.appendChild(box);
    }
  }

  /// 数字/布尔在 JSON 里都是基本类型; 据此决定额外行用哪种控件。
  function extraInputFor(value) {
    if (typeof value === "boolean") return { kind: "bool" };
    if (typeof value === "number") {
      return { kind: Number.isInteger(value) ? "i64" : "f64" };
    }
    return { kind: "string" };
  }

  function renderDetail(id, detail, source, g) {
    st.detailCtx = { id: id, detail: detail, source: source, builtin: source.instance_toml === null && detail.current === null && detail.pair === null };
    // 注: 内置判定以 source 响应的 instance_toml=null 为准(用户策略理论上恒有实例文件)。
    st.detailCtx.builtin = source.instance_toml === null;

    $("sg-d-name").textContent = detail.name || id;
    renderBadges(detail, source);
    renderDeclHint(detail);
    renderFit(detail);

    // 内置只读提示 + 编辑/AI 控件收起(回测保留)。
    const builtin = st.detailCtx.builtin;
    $("sg-builtin-hint").hidden = !builtin;
    $("sg-builtin-hint").textContent = t("sgBuiltinHint");
    $("sg-code").readOnly = builtin;
    $("sg-save").hidden = builtin;
    $("sg-save-top").hidden = builtin;
    $("sg-ai-card").style.display = builtin ? "none" : "";

    renderParams(detail);
    $("sg-pair").value = detail.pair || DEFAULT_PAIR;
    // 053: 交易对基线必须等上面赋完值再记 —— renderParams 建输入框那会儿它还是空的。
    st.savedPair = $("sg-pair").value;
    // 参数清单编辑器(P2-8): 用户策略可声明/维护参数 schema; 内置策略该卡隐藏。
    renderManifestEditor(detail);

    // savedCode 必须取 textarea getter 规范化后的值(CRLF→LF): 直接存服务端原文会让
    // CRLF 策略(Windows 全部)在 updateDirty 里恒判 dirty(「有未保存的修改」常亮)。
    $("sg-code").value = source.lua || "";
    st.savedCode = $("sg-code").value;
    st.aiDraft = false;
    updateDirty();
    setMsg($("sg-savemsg"), "", "");

    // 回测表单 pair 跟随当前实例, 周期/天数保留用户上次输入。
    if (!$("sg-bt-pair").value) $("sg-bt-pair").value = detail.pair || DEFAULT_PAIR;

    // 参数寻优下拉(P2-7): 参数键来自清单注册表 + 实例当前值(去重)。
    fillSweepParams();
    // C8: 多维寻优的维度行(同一份参数键; 空则铺两条)。
    syncOptDims();
    // 编辑器高亮层(P1-4)随内容初始化。
    refreshHighlight();
    $("sg-warnlist").hidden = true;

    // 恢复该策略的在途/近期作业(模块台账, 不跨刷新)。
    const job = st.jobs.get(id);
    if (job) {
      if (job.status === "running") startPolling(id, job.jobId, g);
      renderJob(job);
    }

    // N3: 对话线程与评估面板**必须在详情渲染后重画** —— 空线程会自己收起来, 但不清就是
    // 上一个策略的气泡留在原地(详情骨架只建一次); 评估面板则依赖 detailCtx(改后取参数要用)。
    renderAiThread();
    syncAssessPanel();
  }

  function renderBadges(detail, source) {
    const box = $("sg-d-badges");
    box.innerHTML = "";
    box.appendChild(
      h("span", "sg-badge", detail.market === "futures" ? t("futures") : t("spot"))
    );
    box.appendChild(
      h("span", "sg-badge", source.instance_toml === null ? t("sgBuiltin") : t("sgUser"))
    );
    // 诚实性徽章(2026-10-05)。
    if (detail.declared === false) {
      box.appendChild(h("span", "sg-badge sg-badge-warn", t("sgUndeclared")));
    }
    if (detail.duplicate_of) {
      const dup = h("span", "sg-badge sg-badge-warn", t("sgDupOf"));
      dup.title = detail.duplicate_of;
      box.appendChild(dup);
    }
  }

  /// 诚实性提示条: 未声明参数清单 / 与内置脚本逐字相同 —— 把后果(缺必填参数停机、
  /// 重复下单)和改法一并说清, 用户不用自己去撞(2026-10-05)。
  function renderDeclHint(detail) {
    const box = $("sg-decl-hint");
    const parts = [];
    if (detail.declared === false) parts.push(t("sgDeclHint"));
    if (detail.duplicate_of) parts.push(t("sgDupHint") + " " + detail.duplicate_of + "。");
    box.textContent = parts.join(" ");
    box.hidden = parts.length === 0;
  }

  /// 适用性卡片(帮助计划第 7 条): 清单里的 `summary / description / suitable / unsuitable`
  /// 此前只在「清单编辑器」可见(新建/复制时才展开), 只读详情页一个字都不显示 —— 用户面对
  /// 8 个内置策略只能靠名字猜该选哪个。这里把同一份数据原样展示在只读详情页, **只展示不编辑**
  /// (编辑仍在清单编辑器, 仅用户策略可用)。
  ///
  /// 数据源 = `GET /api/strategies/{id}` 的 manifest, 后端零改动。清单文本是**作者中文原文**
  /// (随策略 TOML 发布), 不随界面语言翻译 —— 与列表里的 `summary` 同一口径, 不另立第二套。
  /// 四栏全空时整卡隐藏(不给一张空壳卡)。
  function renderFit(detail) {
    const card = $("sg-fit-card");
    const body = $("sg-fit-body");
    body.innerHTML = "";
    const rows = [];
    if (detail.summary) rows.push({ k: "sgFitSummary", v: detail.summary });
    if (detail.description) rows.push({ k: "sgFitDesc", v: detail.description });
    if (detail.suitable) rows.push({ k: "sgFitSuitable", v: detail.suitable, kind: "fit-good" });
    if (detail.unsuitable) {
      rows.push({ k: "sgFitUnsuitable", v: detail.unsuitable, kind: "fit-bad" });
    }
    if (!rows.length) {
      card.hidden = true;
      return;
    }
    card.hidden = false;
    for (const r of rows) {
      const row = h("div", "sg-fit-row" + (r.kind ? " " + r.kind : ""));
      const label = h("span", "sg-fit-label", t(r.k));
      // 双语走既有 data-zh/data-en 机制(applyLang 直接刷 textContent)。
      label.setAttribute("data-zh", R.TEXT.zh[r.k]);
      label.setAttribute("data-en", R.TEXT.en[r.k]);
      // 服务端文本一律 textContent(见 h()); description 里的换行由 CSS `pre-wrap` 呈现。
      row.appendChild(label);
      row.appendChild(h("div", "sg-fit-value", r.v));
      body.appendChild(row);
    }
  }

  // ---------- 参数表单 ----------

  function paramRow(spec, value) {
    const label = h("label", "sg-param");
    const head = h("span", "sg-param-head");
    head.appendChild(h("span", "sg-param-name", spec.name || spec.key));
    head.appendChild(h("span", "sg-param-key", spec.key));
    if (spec.required) head.appendChild(h("span", "sg-req", "* " + t("sgRequired")));
    label.appendChild(head);

    // 局部控件统一命名 ctl: 前端禁词扫描不允许给名为 input 的变量做点号 type 赋值(见 web/mod.rs 扫描测试)。
    let ctl;
    if (spec.ty === "bool") {
      ctl = document.createElement("input");
      ctl.type = "checkbox";
      ctl.checked = value === true;
      const wrap = h("div", "sg-param-check");
      wrap.appendChild(ctl);
      label.appendChild(wrap);
    } else if (spec.ty === "enum") {
      ctl = document.createElement("select");
      ctl.className = "sg-input";
      // 空选项(用默认值时不覆盖)。
      const blank = document.createElement("option");
      blank.value = "";
      blank.textContent = "—";
      ctl.appendChild(blank);
      for (const opt of spec.options || []) {
        const o = document.createElement("option");
        o.value = opt;
        o.textContent = opt;
        if (value === opt) o.selected = true;
        ctl.appendChild(o);
      }
      label.appendChild(ctl);
    } else {
      ctl = document.createElement("input");
      ctl.className = "sg-input";
      ctl.type = "number";
      ctl.step = spec.ty === "i64" ? "1" : "any";
      if (spec.ty === "i64") ctl.min = "0";
      if (value !== undefined && value !== null) ctl.value = String(value);
      label.appendChild(ctl);
    }
    if (spec.desc) label.appendChild(h("div", "sg-param-desc", spec.desc));
    return { label: label, input: ctl };
  }

  /// string 类型单独一个分支(text 输入); 与 number 分开避免拼错 type 属性。
  function stringRow(spec, value) {
    const label = h("label", "sg-param");
    const head = h("span", "sg-param-head");
    head.appendChild(h("span", "sg-param-name", spec.name || spec.key));
    head.appendChild(h("span", "sg-param-key", spec.key));
    if (spec.required) head.appendChild(h("span", "sg-req", "* " + t("sgRequired")));
    label.appendChild(head);
    const ctl = document.createElement("input");
    ctl.className = "sg-input";
    ctl.type = "text";
    ctl.autocomplete = "off";
    if (value !== undefined && value !== null) ctl.value = String(value);
    label.appendChild(ctl);
    if (spec.desc) label.appendChild(h("div", "sg-param-desc", spec.desc));
    return { label: label, input: ctl };
  }

  /// 053: 参数行登记项。除了原有的 key/ty/required/input, 多带三样:
  ///   - `name`: 改动清单里显示中文名(没有就回退键名, 与行内标签同一口径);
  ///   - `base`: 基线值(渲染时的字符串), 与当前值比才知道"改没改";
  ///   - `diff`: 行内「原 X → 改为 Y」的容器, 挂在 label 末尾, 未改动时 hidden。
  /// 监听绑 `input` 与 `change` 两个事件: 文本框/数字框两个都发, 下拉与复选框只发 `change`。
  function paramEntry(key, name, ty, required, built) {
    const diff = h("div", "sg-param-diff");
    diff.hidden = true;
    built.label.appendChild(diff);
    built.input.addEventListener("input", onParamInput);
    built.input.addEventListener("change", onParamInput);
    const entry = { key: key, name: name || key, ty: ty, required: !!required, input: built.input, diff: diff, base: "" };
    entry.base = paramTextOf(entry);
    return entry;
  }

  /// 053: 控件当前值的**字符串**表示 —— 参数是否改动一律按字符串比, 不转 Number。
  ///
  /// 为什么用字符串: ① `1` 与 `1.0` 对用户是同一个值, 转数字再比会误报改动;
  /// ② 用户正在输入 `0.` 这种半成品时 `Number()` 得到 NaN, 转数字会把"还在打字"判成改动;
  /// ③ 非法输入会在 `readParam` 里抛错, 而标脏不该抛错。字符串比只看"你看到的是不是变了"。
  function paramTextOf(p) {
    return p.ty === "bool" ? String(!!p.input.checked) : String(p.input.value);
  }

  /// 053: 空值显示成"(空)"而不是一片空白 —— 否则「原 (空) → 1000」看起来像界面漏渲染了。
  function fmtParamVal(v) {
    return v === "" ? t("sgParamEmpty") : v;
  }

  /// 053: 当前表单里**真的变了**的项(参数行 + 交易对)。
  ///
  /// 基线 `p.base` 在渲染时取一次, 保存成功后刷新(`markParamBaseline`) —— 所以"改了又改回去"
  /// 不算改动, 与代码侧的 `savedCode` 同一口径。交易对同样只在保存时落盘, 一并纳入。
  function paramDiffs() {
    const out = [];
    for (const p of st.paramInputs) {
      if (!p.input) continue;
      const now = paramTextOf(p);
      if (now === p.base) continue;
      out.push({ key: p.key, name: p.name || p.key, before: p.base, after: now });
    }
    const pairEl = $("sg-pair");
    if (pairEl && typeof st.savedPair === "string" && pairEl.value !== st.savedPair) {
      out.push({ key: "pair", name: t("sgPair"), before: st.savedPair, after: pairEl.value });
    }
    return out;
  }

  /// 053: 保存成功后把基线推进到"刚落盘的那组值" —— 否则保存完还亮着未保存提示。
  function markParamBaseline() {
    for (const p of st.paramInputs) {
      if (p.input) p.base = paramTextOf(p);
    }
    const pairEl = $("sg-pair");
    if (pairEl) st.savedPair = pairEl.value;
  }

  /// 053: 改动对比。两处一起刷 —— 参数行**行内**标出「原 X → 改为 Y」(改哪项一眼看到),
  /// 卡片顶部另给一份**汇总清单**(参数多时要滚屏, 行内会看不见) + 一键还原。
  function renderParamDiff() {
    const diffs = paramDiffs();
    const byKey = new Map();
    for (const d of diffs) byKey.set(d.key, d);
    for (const p of st.paramInputs) {
      if (!p.diff) continue;
      const d = byKey.get(p.key);
      p.diff.textContent = "";
      if (!d) {
        p.diff.hidden = true;
        continue;
      }
      p.diff.hidden = false;
      p.diff.appendChild(h("span", "sg-diff-old", t("sgParamFrom") + " " + fmtParamVal(d.before)));
      p.diff.appendChild(h("span", "sg-diff-arrow", "→"));
      p.diff.appendChild(h("span", "sg-diff-new", t("sgParamTo") + " " + fmtParamVal(d.after)));
    }
    renderChangesPanel(diffs);
  }

  function renderChangesPanel(diffs) {
    const box = $("sg-param-changes");
    if (!box) return;
    box.innerHTML = "";
    if (!diffs.length) {
      box.hidden = true;
      return;
    }
    box.appendChild(h("div", "sg-changes-title", t("sgParamChangesTitle")));
    const list = h("div", "sg-changes-list");
    for (const d of diffs) {
      const row = h("div", "sg-change-row");
      row.appendChild(h("span", "sg-change-name", d.name));
      row.appendChild(h("span", "sg-change-key", d.key));
      const val = h("span", "sg-change-val");
      val.appendChild(h("span", "sg-diff-old", fmtParamVal(d.before)));
      val.appendChild(h("span", "sg-diff-arrow", " → "));
      val.appendChild(h("span", "sg-diff-new", fmtParamVal(d.after)));
      row.appendChild(val);
      list.appendChild(row);
    }
    box.appendChild(list);
    const btn = h("button", "sg-btn sg-btn-mini", t("sgParamRevert"));
    btn.type = "button";
    btn.id = "sg-param-revert";
    btn.addEventListener("click", onRevertParams);
    box.appendChild(btn);
    box.hidden = false;
  }

  /// 053: 把参数(含交易对)还原到**基线**。代码另有「撤销」按钮, 各管各的 ——
  /// 混成一次撤销会让用户以为顺手把代码也退回了(或反过来)。
  function onRevertParams() {
    for (const p of st.paramInputs) {
      if (!p.input) continue;
      if (p.ty === "bool") p.input.checked = p.base === "true";
      else p.input.value = p.base;
    }
    const pairEl = $("sg-pair");
    if (pairEl && typeof st.savedPair === "string") pairEl.value = st.savedPair;
    updateDirty();
  }

  /// 053: 参数控件改动 → 重算未保存状态。此前只有代码编辑器有 `keydown`/`refreshHighlight`
  /// 触发标脏, 参数框改值**没有任何监听** —— 这是"改了参数界面不吭声"的直接原因。
  function onParamInput() {
    updateDirty();
  }

  function renderParams(detail) {
    st.paramInputs = [];
    const box = $("sg-params");
    box.innerHTML = "";

    const pairField = h("label", "sg-param");
    pairField.appendChild(h("span", "sg-param-head", t("sgPair")));
    const pairInput = document.createElement("input");
    pairInput.className = "sg-input";
    pairInput.id = "sg-pair";
    pairInput.type = "text";
    pairInput.autocomplete = "off";
    pairInput.addEventListener("input", onParamInput);
    pairField.appendChild(pairInput);
    box.appendChild(pairField);

    const current = detail.current || {};
    const known = new Set();

    for (const p of detail.params || []) {
      // 保留键跳过: `pair` 上面已独立成 `#sg-pair` 字段, `script`/`script_path` 指脚本本身 ——
      // 再来一行就是被顶层值盖掉的死控件(2026-10-05 修)。
      if (RESERVED_PARAM_KEYS.includes(p.key)) continue;
      known.add(p.key);
      const built = p.ty === "string" ? stringRow(p, current[p.key] !== undefined ? current[p.key] : p.default)
        : paramRow(p, current[p.key] !== undefined ? current[p.key] : p.default);
      box.appendChild(built.label);
      st.paramInputs.push(paramEntry(p.key, p.name, p.ty, p.required, built));
    }

    // 实例里存在但清单没声明的键(网页保存的 lua 策略没有清单): 也给可编辑行, 避免保存时黑箱丢失
    // (即使不渲染, 保存也会以 current 为基线保留; 这里让人能改)。
    const extra = Object.keys(current)
      .filter((k) => !known.has(k))
      .sort();
    if (extra.length) {
      box.appendChild(h("div", "sg-params-subhead", t("sgOtherParams")));
      // 052: 这些键在实例 TOML 里有值、清单里没声明 —— 它们**没有中文名与说明**。
      // 不解释清楚的话, 用户会以为这就是全部参数, 或者反过来以为界面坏了。
      box.appendChild(h("div", "sg-hint", t("sgOtherParamsHint")));
      for (const key of extra) {
        const spec0 = { key: key, name: key, ty: extraInputFor(current[key]).kind, desc: "", required: false, options: null };
        const built = spec0.ty === "string" ? stringRow(spec0, current[key]) : paramRow(spec0, current[key]);
        box.appendChild(built.label);
        st.paramInputs.push(paramEntry(key, key, spec0.ty, false, built));
      }
    }

    // 052 方案 4: 一行都没画出来时(清单为空)给引导卡 —— 只丢一句红字用户不知道该干什么。
    renderParamsGuide(detail);
  }

  /// 052 方案 4: 空参数表单的引导卡。
  ///
  /// 出现条件 = **清单里没有任何可渲染的参数**(全是保留键也算空): 这正是"复制完只剩一个交易对框"
  /// 的那个状态。判据取自服务端返回的 `detail.params`, 不看 DOM, 也不猜。
  /// 内置策略不显示 —— 它的参数说明随代码发布, 不存在"没整理出来", 且编辑入口整卡已隐藏。
  /// 两个动作分别对应方案 2(静态识别)与方案 3(问 AI 调参); 后者需要清单先存在, 所以清单为空时
  /// 它只把用户推去走第一步, **不假装能直接问**(后端 `mode=params` 清单为空时也是 400)。
  function renderParamsGuide(detail) {
    const box = $("sg-params-guide");
    if (!box) return;
    box.innerHTML = "";
    const declared = (detail.params || []).filter(
      (p) => !RESERVED_PARAM_KEYS.includes(p.key)
    ).length;
    if (declared > 0 || (st.detailCtx && st.detailCtx.builtin)) {
      box.hidden = true;
      return;
    }
    box.appendChild(h("div", "sg-guide-title", t("sgParamsGuideTitle")));
    box.appendChild(h("div", "sg-guide-body", t("sgParamsGuideBody")));
    const actions = h("div", "sg-guide-actions");
    const btnExtract = h("button", "sg-btn sg-btn-mini", t("sgParamsGuideExtract"));
    btnExtract.type = "button";
    btnExtract.addEventListener("click", () => onExtractParams());
    const btnAi = h("button", "sg-btn sg-btn-mini", t("sgParamsGuideAskAi"));
    btnAi.type = "button";
    btnAi.addEventListener("click", onGuideAskAi);
    actions.append(btnExtract, btnAi);
    box.appendChild(actions);
    const msg = h("div", "sg-msg");
    msg.id = "sg-params-guide-msg";
    box.appendChild(msg);
    box.hidden = false;
  }

  /// 引导卡的「问 AI 怎么配」: AI 的调参模式只认**清单里声明过的键** ——
  /// 清单还空着就先去识别, 别让用户发一条注定 400 的请求(后端也会这么回, 但那是撞墙不是引导)。
  function onGuideAskAi() {
    const msg = $("sg-params-guide-msg");
    if (!st.manifestRows || !st.manifestRows.length) {
      setMsg(msg, t("sgParamsGuideAskAiFirst"), "err");
      return;
    }
    setMsg(msg, "", "");
    setAiMode("params");
    const card = $("sg-ai-card");
    if (card && card.scrollIntoView) card.scrollIntoView({ block: "center" });
    $("sg-ai-instr").focus();
  }

  /// 读一个参数控件: 空 number/string → undefined(不覆盖); 非法数字抛错(带参数名)。
  function readParam(p) {
    const el = p.input;
    if (p.ty === "bool") return el.checked;
    const raw = el.value.trim();
    if (raw === "") return undefined;
    if (p.ty === "string" || p.ty === "enum") return raw;
    const n = Number(raw);
    if (!Number.isFinite(n)) throw new Error((p.key + ": ") + t("sgNumberInvalid"));
    if (p.ty === "i64") {
      if (!Number.isInteger(n)) throw new Error((p.key + ": ") + t("sgNumberInvalid"));
      return n;
    }
    return n;
  }

  /// 收集参数: 以实例 current 为基线(保住清单未声明的键), 表单值覆盖。
  /// requireFilled=true(保存)时校验必填; false(回测)时空值跳过。
  function gatherParams(requireFilled) {
    const out = {};
    if (st.detailCtx && st.detailCtx.detail.current) Object.assign(out, st.detailCtx.detail.current);
    for (const p of st.paramInputs) {
      const v = readParam(p);
      if (v === undefined) {
        if (requireFilled && p.required && p.ty !== "bool") {
          throw new Error(t("sgRequiredEmpty") + ": " + p.key);
        }
        continue;
      }
      out[p.key] = v;
    }
    return out;
  }

  // ---------- 参数清单 (manifest) 编辑 (P2-8) ----------
  //
  // 用户自写策略默认没有清单 → 参数表单空缺。这里让用户为自己的策略声明参数 schema,
  // 落盘 strategies/{market}/{id}.toml (与内置清单同构同目录), catalog 扫描后参数表单随即点亮。

  /// 清单编辑里的当前参数行(保存时逐行收集; 不含空行)。

  /// 小号文本输入(清单行内的紧凑字段)。
  function mfInput(cls, value, placeholder) {
    const el = document.createElement("input");
    el.className = "sg-input " + cls;
    el.type = "text";
    el.autocomplete = "off";
    el.spellcheck = false;
    if (value) el.value = value;
    if (placeholder) el.placeholder = placeholder;
    return el;
  }

  /// 渲染清单编辑器(仅用户策略): 顶层元数据 + 参数行。内置策略整卡隐藏。
  function renderManifestEditor(detail) {
    const card = $("sg-manifest-card");
    if (!card) return;
    if (!st.detailCtx || st.detailCtx.builtin) {
      card.hidden = true;
      return;
    }
    card.hidden = false;
    $("sg-mf-name").value = detail.name || "";
    $("sg-mf-summary").value = detail.summary || "";
    $("sg-mf-suitable").value = detail.suitable || "";
    $("sg-mf-unsuitable").value = detail.unsuitable || "";
    $("sg-mf-desc").value = detail.description || "";
    const box = $("sg-mf-params");
    box.innerHTML = "";
    st.manifestRows = [];
    for (const p of detail.params || []) addManifestRow(p);
    setMsg($("sg-mf-msg"), st.manifestRows.length ? "" : t("sgMfEmpty"), "");
  }

  /// 052 方案 2: 从 Lua 源码**静态识别**参数 → 预填进清单编辑器(不落盘)。
  ///
  /// 吃的是**编辑器里的当前代码**(可能是未保存的草稿): 端点是**无状态**的 ——
  /// "这份脚本读了哪些参数"是代码本身的属性, 要求用户先保存再识别是多余的一步。
  /// 全程不碰 AI: 识别结果只是**键 / 类型 / 代码里的字面量兜底默认**, 中文名留给用户或 AI 补。
  async function onExtractParams() {
    if (!st.detailCtx) return;
    const code = $("sg-code").value;
    const btn = $("sg-mf-extract");
    if (!code.trim()) {
      setMsg($("sg-mf-msg"), t("sgCodeEmpty"), "err");
      return;
    }
    const g = st.gen;
    btn.disabled = true;
    btn.textContent = t("sgMfExtracting");
    try {
      const reply = await R.api("/api/strategies/extract-params", {
        method: "POST",
        body: { code: code },
      });
      if (g !== st.gen) return;
      const added = applyExtractedParams(reply.params || []);
      setMsg($("sg-mf-msg"), added ? t("sgMfExtractOk") : t("sgMfExtractNone"), added ? "ok" : "");
      if (added) {
        // 刚填好行却还收在折叠框里 —— 用户看不到发生了什么。替他展开。
        const det = $("sg-manifest-card").querySelector("details");
        if (det) det.open = true;
      }
    } catch (err) {
      if (g !== st.gen) return;
      setMsg($("sg-mf-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === st.gen) {
        btn.disabled = false;
        btn.textContent = t("sgMfExtract");
      }
    }
  }

  /// 把识别结果并进清单编辑器, 返回**新增**行数(0 = 一个键都没新加)。
  ///
  /// 已存在的键**跳过不覆盖** —— 用户可能已经填过中文名/说明/默认值, 识别结果没有资格盖掉人工输入。
  /// 类型与默认值**照抄**识别结果(`num`/`config_f64` → f64、字面量兜底值), 不美化也不猜。
  function applyExtractedParams(list) {
    const have = new Set(st.manifestRows.map((r) => r.key.value.trim()).filter((k) => k));
    let added = 0;
    for (const p of list) {
      if (!p || !p.key || have.has(p.key)) continue;
      addManifestRow({
        key: p.key,
        name: "",
        ty: p.type || "f64",
        default: p.default,
        // N2 (2026-10-12): 说明优先用**脚本注释里原样搬来的那句**(静态提取带回来的 `desc`)。
        // 中文名**仍然留空** —— 注释首段不一定是名字(`--  pair  必填, 交易对(现货)` 的首段是
        // 「必填」), 切错比留空糟; 参数表在 name 为空时回退显示键名, 想要更好的名字可按「AI 起名」。
        desc: p.desc || "",
        required: false,
        options: null,
      });
      have.add(p.key);
      added += 1;
    }
    return added;
  }

  /// N2-C: 让 AI 按**脚本代码**给清单里的参数键起中文名与写说明(不落盘)。
  ///
  /// 与"识别参数"的分工: 识别回答"有哪些键", 起名回答"这些键叫什么、干什么"。
  /// 键集合由**服务端静态提取**决定(不是让模型自己列 —— 那会多出编造的键), 所以这里只把 AI
  /// 给的名字**填回已存在的行**; 代码里读不到、清单里却有的键, AI 根本不会收到它。
  async function onAiNameParams() {
    if (!st.detailCtx) return;
    const code = $("sg-code").value;
    const btn = $("sg-mf-autoname");
    if (!code.trim()) {
      setMsg($("sg-mf-msg"), t("sgCodeEmpty"), "err");
      return;
    }
    const g = st.gen;
    btn.disabled = true;
    btn.textContent = t("sgMfAutoNaming");
    try {
      const reply = await R.api("/api/strategies/ai-name-params", {
        method: "POST",
        body: { code: code },
      });
      if (g !== st.gen) return;
      const filled = applyAiNames(reply.names || []);
      setMsg($("sg-mf-msg"), filled ? t("sgMfNameOk") : t("sgMfNameNone"), filled ? "ok" : "");
      if (filled) {
        const det = $("sg-manifest-card").querySelector("details");
        if (det) det.open = true;
      }
    } catch (err) {
      if (g !== st.gen) return;
      setMsg($("sg-mf-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === st.gen) {
        btn.disabled = false;
        btn.textContent = t("sgMfAutoName");
      }
    }
  }

  /// 把 AI 给的名字/说明填进清单行, 返回**真正填上**的行数。
  ///
  /// 只填**空着的**格子: 已有内容一个字都不动(与 `applyExtractedParams` 的"跳过不覆盖"同一
  /// 纪律 —— AI 没资格覆盖人工输入)。说明也同样只填空位: 从注释里带出来的那句(识别参数时已填)
  /// 优先于 AI 现编的。
  function applyAiNames(list) {
    const byKey = new Map();
    for (const n of list) {
      if (n && n.key) byKey.set(n.key, n);
    }
    let filled = 0;
    for (const r of st.manifestRows) {
      const n = byKey.get(r.key.value.trim());
      if (!n) continue;
      let touched = false;
      if (!r.name.value.trim() && n.name) {
        r.name.value = n.name;
        touched = true;
      }
      if (!r.desc.value.trim() && n.desc) {
        r.desc.value = n.desc;
        touched = true;
      }
      if (touched) filled += 1;
    }
    return filled;
  }

  /// 追加一个参数行(p 为 null 时空行)。type 为 enum 时展开"可选值"输入。
  function addManifestRow(p) {
    const spec = p || {};
    const row = h("div", "sg-mf-row");

    const key = mfInput("sg-mf-k", spec.key || "", t("sgMfKeyPh"));
    const name = mfInput("sg-mf-n", spec.name || "", t("sgMfNamePh"));
    const ty = document.createElement("select");
    ty.className = "sg-input sg-mf-ty";
    for (const v of ["f64", "i64", "string", "bool", "enum"]) {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = v;
      ty.appendChild(o);
    }
    ty.value = spec.ty || "f64";
    const defVal =
      spec.default === undefined || spec.default === null ? "" : String(spec.default);
    const def = mfInput("sg-mf-def", defVal, t("sgMfDefaultPh"));

    const reqWrap = h("label", "sg-mf-reqwrap");
    const req = document.createElement("input");
    req.type = "checkbox";
    req.checked = !!spec.required;
    reqWrap.appendChild(req);
    reqWrap.appendChild(document.createTextNode(t("sgMfRequired")));

    const del = h("button", "sg-btn sg-btn-mini sg-mf-del", t("sgMfDel"));
    del.type = "button";

    const line1 = h("div", "sg-mf-line");
    line1.append(key, name, ty, def, reqWrap, del);

    const desc = mfInput("sg-mf-d", spec.desc || "", t("sgMfDescPh"));
    const opts = mfInput("sg-mf-o", (spec.options || []).join(","), t("sgMfOptionsPh"));
    const line2 = h("div", "sg-mf-line2");
    line2.append(desc, opts);

    const syncOpts = () => {
      opts.style.display = ty.value === "enum" ? "" : "none";
    };
    ty.addEventListener("change", syncOpts);
    syncOpts();

    del.addEventListener("click", () => {
      st.manifestRows = st.manifestRows.filter((r) => r.row !== row);
      row.remove();
    });

    row.append(line1, line2);
    st.manifestRows.push({ row: row, key: key, name: name, ty: ty, desc: desc, def: def, req: req, opts: opts });
    $("sg-mf-params").appendChild(row);
    return row;
  }

  /// 收集清单表单(空行跳过; 默认值按类型转成 JSON 数字/布尔/字符串)。
  function collectManifest() {
    const params = [];
    for (const r of st.manifestRows) {
      const key = r.key.value.trim();
      const name = r.name.value.trim();
      if (!key && !name) continue;
      const ty = r.ty.value;
      const p = { key: key, name: name, type: ty, desc: r.desc.value.trim(), required: r.req.checked };
      const raw = r.def.value.trim();
      if (raw !== "") {
        if (ty === "f64") p.default = Number(raw);
        else if (ty === "i64") p.default = Number(raw);
        else if (ty === "bool") p.default = raw === "true" || raw === "1";
        else p.default = raw;
      }
      if (ty === "enum") {
        p.options = r.opts.value
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s);
      }
      params.push(p);
    }
    return {
      name: $("sg-mf-name").value.trim(),
      summary: $("sg-mf-summary").value.trim(),
      description: $("sg-mf-desc").value.trim(),
      suitable: $("sg-mf-suitable").value.trim(),
      unsuitable: $("sg-mf-unsuitable").value.trim(),
      params: params,
    };
  }

  /// 保存清单 → 重取详情 → 参数表单/寻优下拉立即按新 schema 重建(不动编辑器里的代码)。
  async function onSaveManifest() {
    if (!st.detailCtx) return;
    const g = st.gen;
    const btn = $("sg-mf-save");
    btn.disabled = true;
    try {
      await R.api("/api/strategies/" + encodeURIComponent(st.detailCtx.id) + "/manifest", {
        method: "POST",
        body: collectManifest(),
      });
      if (g !== st.gen) return;
      setMsg($("sg-mf-msg"), t("sgMfSaved"), "ok");
      await refreshSchema(g);
    } catch (err) {
      if (g !== st.gen) return;
      setMsg($("sg-mf-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === st.gen) btn.disabled = false;
    }
  }

  /// 重取详情并只重建"清单相关"的视图(参数表单 + 清单编辑器 + 寻优参数下拉),
  /// 刻意不碰代码编辑器(避免丢掉未保存的代码草稿)。
  async function refreshSchema(g) {
    const id = st.detailCtx && st.detailCtx.id;
    if (!id) return;
    const detail = await R.api("/api/strategies/" + encodeURIComponent(id));
    if (g !== st.gen || !st.detailCtx || st.detailCtx.id !== id) return;
    st.detailCtx.detail = detail;
    renderParams(detail);
    $("sg-pair").value = detail.pair || $("sg-pair").value || DEFAULT_PAIR;
    // 053: 同上 —— 重画表单后基线要跟着服务端这份值重记(保存清单后参数键可能整批换过)。
    st.savedPair = $("sg-pair").value;
    renderManifestEditor(detail);
    fillSweepParams();
    fillOptDims();
  }

  /// 寻优参数下拉: 键来自当前参数表单(清单 + 实例值), 去重。
  function fillSweepParams() {
    const sel = $("sg-sw-param");
    if (!sel) return;
    const prev = sel.value;
    sel.innerHTML = "";
    for (const key of paramKeys()) {
      const o = document.createElement("option");
      o.value = key;
      o.textContent = key;
      sel.appendChild(o);
    }
    if (prev && paramKeys().includes(prev)) sel.value = prev;
  }

  // ---------- C8 多维寻优: 维度行 ----------

  /// 维度上限 —— **必须与后端 `web::backtest_jobs::MAX_OPTIMIZE_DIMS` 一致**(4)。
  /// 后端那道是权威(超了会 400); 这道只是提前把按钮按掉, 免得用户填完 5 行才被拒。
  const OPT_MAX_DIMS = 4;

  /// 参数键列表(清单 + 实例值, 去重)。单参数下拉与多维寻优的维度行**共用这一份** ——
  /// 否则两处会各自列表, 加了个参数只在一处出现。
  function paramKeys() {
    const seen = new Set();
    const out = [];
    for (const p of st.paramInputs) {
      if (seen.has(p.key)) continue;
      seen.add(p.key);
      out.push(p.key);
    }
    return out;
  }

  /// 选中项优先保留(重刷时别把用户刚选的键弹掉), 找不到才退回第一项。
  function fillSelect(sel, keys) {
    const prev = sel.value;
    sel.textContent = "";
    for (const key of keys) {
      const o = document.createElement("option");
      o.value = key;
      o.textContent = key;
      sel.appendChild(o);
    }
    if (prev && keys.includes(prev)) sel.value = prev;
  }

  /// 重刷所有维度行的键下拉(参数清单变了之后调用, 与 `fillSweepParams` 同时机)。
  function fillOptDims() {
    const box = $("sg-opt-dims");
    if (!box) return;
    const keys = paramKeys();
    for (const sel of box.querySelectorAll(".sg-opt-key")) fillSelect(sel, keys);
  }

  /// 详情渲染后调用: 一条维度行都没有就先铺两条(默认就是"两个参数比一比"的场景),
  /// 然后刷新所有键下拉。**不放在 mount 里** —— 那时策略详情还没读, 参数清单是空的。
  function syncOptDims() {
    const box = $("sg-opt-dims");
    if (!box) return;
    if (!box.querySelectorAll(".sg-opt-dim").length && paramKeys().length) {
      addOptDim();
      addOptDim();
    }
    fillOptDims();
  }

  /// 维度行满了就把「添加维度」按掉 —— 按钮禁用比点了报错更早一步。
  function syncOptAddBtn() {
    const box = $("sg-opt-dims");
    const btn = $("sg-opt-add");
    if (!box || !btn) return;
    btn.disabled = box.querySelectorAll(".sg-opt-dim").length >= OPT_MAX_DIMS;
  }

  /// 新增一条维度行: 键下拉 + 候选值输入 + 删除。空装置(没有任何参数)时给一条提示, 不静默加空行。
  function addOptDim() {
    const box = $("sg-opt-dims");
    if (!box) return;
    if (box.querySelectorAll(".sg-opt-dim").length >= OPT_MAX_DIMS) {
      setMsg($("sg-opt-msg"), t("sgOptMaxDims"), "err");
      return;
    }
    const keys = paramKeys();
    if (!keys.length) {
      setMsg($("sg-opt-msg"), t("sgSweepEmpty"), "err");
      return;
    }
    const row = h("div", "sg-opt-dim");
    const sel = h("select", "sg-input sg-opt-key");
    fillSelect(sel, keys);
    row.appendChild(sel);
    const vals = h("input", "sg-input sg-opt-values");
    // 输入框的 `type` 用 setAttribute 设(直接属性赋值会被前端纪律扫描当成 `<textarea>.type=`)。
    vals.setAttribute("type", "text");
    vals.setAttribute("autocomplete", "off");
    vals.setAttribute("data-zh-placeholder", R.TEXT.zh.sgOptValues);
    vals.setAttribute("data-en-placeholder", R.TEXT.en.sgOptValues);
    vals.placeholder = t("sgOptValues");
    row.appendChild(vals);
    const del = h("button", "sg-btn sg-btn-mini sg-opt-del", t("sgOptDel"));
    del.type = "button";
    del.setAttribute("data-zh", R.TEXT.zh.sgOptDel);
    del.setAttribute("data-en", R.TEXT.en.sgOptDel);
    del.addEventListener("click", () => {
      row.remove();
      syncOptAddBtn();
    });
    row.appendChild(del);
    box.appendChild(row);
    syncOptAddBtn();
  }

  /// 读回维度行 → 请求体的 `params` 数组。空键/空值当场报错(别把一个空维度发上去等后端拒)。
  /// 值按"像数字就当数字"解析 —— 与单参数扫描同一手感(后端按策略清单的声明类型兜底)。
  function readOptDims() {
    const box = $("sg-opt-dims");
    const out = [];
    if (!box) return out;
    for (const row of box.querySelectorAll(".sg-opt-dim")) {
      const key = row.querySelector(".sg-opt-key").value.trim();
      const raw = row.querySelector(".sg-opt-values").value.trim();
      if (!key || !raw) return null;
      out.push({
        key: key,
        values: raw
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean)
          .map((s) => (Number.isFinite(Number(s)) && s !== "" ? Number(s) : s)),
      });
    }
    return out;
  }

  // ---------- 对外(外壳与其它子文件调用) ----------
  S.enterDetail = enterDetail;
  S.renderBadges = renderBadges;
  S.readParam = readParam;
  S.paramRow = paramRow;
  S.stringRow = stringRow;
  S.addManifestRow = addManifestRow;
  S.onSaveManifest = onSaveManifest;
  // 052: 「从脚本识别参数」的按钮在**详情骨架**里(mount 建一次) —— 回调必须导出,
  // 否则挂上去的是 undefined, 点了整片没反应(042 那批踩过同一个坑)。
  S.onExtractParams = onExtractParams;
  // N2-C: 同上 —— 「让 AI 起中文名」按钮也在骨架里, 漏导出就是点了没反应。
  S.onAiNameParams = onAiNameParams;
  // 052: 切语言时外壳要就地重建引导卡(它的文字是动态建的), 故一并导出。
  S.renderParamsGuide = renderParamsGuide;
  S.gatherParams = gatherParams;
  // 053: 参数侧标脏三件套 —— editor 子文件的 `updateDirty` 要问"参数改没改", 保存成功后要
  // 把基线推进到刚落盘那组值, 两者都只经这三个入口, 不直接碰 `st.paramInputs`。
  S.paramDiffs = paramDiffs;
  S.markParamBaseline = markParamBaseline;
  S.renderParamDiff = renderParamDiff;
  // C8: 维度行的建 / 刷 / 读 —— 外壳 mount 用 addOptDim, 回测子文件用 readOptDims。
  S.addOptDim = addOptDim;
  S.fillOptDims = fillOptDims;
  S.syncOptDims = syncOptDims;
  S.readOptDims = readOptDims;
})();
