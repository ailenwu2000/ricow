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

  // ---------- 详情态 ----------

  async function enterDetail(id, g) {
    st.detailCtx = null;
    st.paramInputs = [];
    st.savedCode = "";
    st.aiDraft = false;
    st.saveLocked = false;
    $("sg-params").innerHTML = "";
    $("sg-code").value = "";
    $("sg-code").readOnly = false;
    $("sg-ai-instr").value = "";
    setMsg($("sg-savemsg"), "", "");
    setMsg($("sg-ai-msg"), "", "");
    setMsg($("sg-bt-msg"), "", "");
    $("sg-bt-result").innerHTML = "";
    $("sg-dirty").hidden = true;
    $("sg-builtin-hint").hidden = true;
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
    // 编辑器高亮层(P1-4)随内容初始化。
    refreshHighlight();
    $("sg-warnlist").hidden = true;

    // 恢复该策略的在途/近期作业(模块台账, 不跨刷新)。
    const job = st.jobs.get(id);
    if (job) {
      if (job.status === "running") startPolling(id, job.jobId, g);
      renderJob(job);
    }
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
      st.paramInputs.push({ key: p.key, ty: p.ty, required: !!p.required, input: built.input });
    }

    // 实例里存在但清单没声明的键(网页保存的 lua 策略没有清单): 也给可编辑行, 避免保存时黑箱丢失
    // (即使不渲染, 保存也会以 current 为基线保留; 这里让人能改)。
    const extra = Object.keys(current)
      .filter((k) => !known.has(k))
      .sort();
    if (extra.length) {
      box.appendChild(h("div", "sg-params-subhead", t("sgOtherParams")));
      for (const key of extra) {
        const spec0 = { key: key, name: key, ty: extraInputFor(current[key]).kind, desc: "", required: false, options: null };
        const built = spec0.ty === "string" ? stringRow(spec0, current[key]) : paramRow(spec0, current[key]);
        box.appendChild(built.label);
        st.paramInputs.push({ key: key, ty: spec0.ty, required: false, input: built.input });
      }
    }
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
    renderManifestEditor(detail);
    fillSweepParams();
  }

  /// 寻优参数下拉: 键来自当前参数表单(清单 + 实例值), 去重。
  function fillSweepParams() {
    const sel = $("sg-sw-param");
    if (!sel) return;
    const prev = sel.value;
    sel.innerHTML = "";
    const seen = new Set();
    for (const p of st.paramInputs) {
      if (seen.has(p.key)) continue;
      seen.add(p.key);
      const o = document.createElement("option");
      o.value = p.key;
      o.textContent = p.key;
      sel.appendChild(o);
    }
    if (prev && seen.has(prev)) sel.value = prev;
  }

  // ---------- 对外(外壳与其它子文件调用) ----------
  S.enterDetail = enterDetail;
  S.renderBadges = renderBadges;
  S.readParam = readParam;
  S.paramRow = paramRow;
  S.stringRow = stringRow;
  S.addManifestRow = addManifestRow;
  S.onSaveManifest = onSaveManifest;
  S.gatherParams = gatherParams;
})();
