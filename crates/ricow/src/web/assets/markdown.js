// Ricow Web 前端「受限 Markdown 渲染器」(044): 把 AI 助手的回答从纯文本变成可读的结构化内容
// —— 围栏代码块 / 行内代码 / 加粗 / 斜体 / 删除线 / 链接 / 标题 / 列表 / 引用 / 水平线 / 表格。
//
// **纪律(与 assets.rs 的扫描一致, 别改)**:
//   1. 本文件**绝不使用 `innerHTML`** —— AI 的回答是**不可信内容**, 全部经
//      `document.createElement` / `textContent` / `createTextNode` 成 DOM。
//      这条不是洁癖: 走 innerHTML 就得维护一套转义 + 白名单, 漏一处就是 XSS;
//      DOM API 路线从结构上就没有注入面。
//   2. 不碰浏览器存储(SC-011)。注意: 纪律扫描是按**字面量**匹配的, 所以这里连那两个
//      存储 API 的名字都不写出来 —— 写出来反而会把扫描弄红。
//   3. 链接只放行 `http:` / `https:`, 其余(`javascript:` / `data:` …)降级成纯文本;
//      外链一律 `target="_blank"` + `rel="noopener noreferrer"`。
//
// 范围: **不是 CommonMark 全实现**(见 `specs/changes/044-webui-ux-polish/spec.md` §三) ——
// 不认识的行原样显示, 不猜。宁少不错: 少认一种语法只是不好看, 认错一种会把用户文本改坏。
"use strict";

(function () {
  const R = (window.Ricow = window.Ricow || {});

  /// 建一个元素; `text` 走 `textContent`(唯一的内容注入方式)。
  function el(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  // ---------- 行内 ----------

  /// 行内语法一次扫描。**顺序即优先级**: 先行内代码(其内部不再解析), 再 `**加粗**`,
  /// 再删除线, 再斜体(`_` 那种要卡词边界, 免得 `some_var_name` 被吃成斜体), 最后链接。
  const RE_INLINE = new RegExp(
    [
      "(`[^`\\n]+`)", // 1 行内代码
      "(\\*\\*[^*\\n]+\\*\\*)", // 2 加粗
      "(~~[^~\\n]+~~)", // 3 删除线
      "((?<![A-Za-z0-9_])_[^_\\n]+_(?![A-Za-z0-9_]))", // 4 斜体(_)
      "(\\*[^*\\n]+\\*)", // 5 斜体(*)
      "(\\[[^\\]\\n]*\\]\\([^)\\s]+\\))", // 6 链接
    ].join("|"),
    "g",
  );

  /// 只放行 http(s) —— `javascript:` 之流降级为纯文本。
  function safeHref(url) {
    return /^https?:\/\//i.test(url) ? url : null;
  }

  /// `[文字](地址)` → `<a>`(或降级为纯文本); 其余 → `<pre>` 之外的对应元素。
  function link(text, url) {
    const href = safeHref(url);
    if (!href) return document.createTextNode(text + "(" + url + ")");
    const a = el("a", "md-link", text);
    a.href = href;
    a.target = "_blank";
    a.rel = "noopener noreferrer";
    return a;
  }

  /// 把一行文本切成行内节点, 追加到 `host`。
  function inline(host, text) {
    RE_INLINE.lastIndex = 0;
    let last = 0;
    let m;
    while ((m = RE_INLINE.exec(text))) {
      if (m.index > last) host.appendChild(document.createTextNode(text.slice(last, m.index)));
      const full = m[0];
      if (m[1]) {
        host.appendChild(el("code", "md-code", full.slice(1, -1)));
      } else if (m[2]) {
        host.appendChild(el("strong", "md-strong", full.slice(2, -2)));
      } else if (m[3]) {
        host.appendChild(el("del", "md-del", full.slice(2, -2)));
      } else if (m[4] || m[5]) {
        host.appendChild(el("em", "md-em", full.slice(1, -1)));
      } else if (m[6]) {
        const parts = /^\[([^\]]*)\]\(([^)]*)\)$/.exec(full);
        host.appendChild(link(parts[1], parts[2]));
      }
      last = m.index + full.length;
    }
    if (last < text.length) host.appendChild(document.createTextNode(text.slice(last)));
    return host;
  }

  function rich(tag, cls, text) {
    return inline(el(tag, cls), text);
  }

  // ---------- 块级 ----------

  const RE_FENCE = /^\s*```(.*)$/;
  const RE_HEAD = /^(#{1,6})\s+(.*)$/;
  const RE_HR = /^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/;
  const RE_QUOTE = /^\s*>\s?(.*)$/;
  const RE_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
  /// 表格分隔行: `|---|:--:|` 之类, 至少一个 `|`。
  const RE_TSEP = /^\s*\|?[\s:|-]*-[\s:|-]*\|[\s:|-]*$/;

  /// 一行是否"块级开头"(用于结束段落收集)。
  function startsBlock(line) {
    return (
      RE_FENCE.test(line) ||
      RE_HEAD.test(line) ||
      RE_HR.test(line) ||
      RE_QUOTE.test(line) ||
      RE_ITEM.test(line) ||
      /^\s*\|/.test(line)
    );
  }

  /// `| a | b |` → `["a","b"]`。
  function cells(line) {
    let s = line.trim();
    if (s.startsWith("|")) s = s.slice(1);
    if (s.endsWith("|")) s = s.slice(0, -1);
    return s.split("|").map((c) => c.trim());
  }

  /// 列表: 收集连续的行, 支持**一级缩进嵌套**(更深的缩进按一级算 —— 见 §三 非目标)。
  function list(lines, start) {
    const ordered = /^\d/.test(RE_ITEM.exec(lines[start])[2]);
    const box = el(ordered ? "ol" : "ul", "md-list");
    let cur = null; // 当前一级条目
    let sub = null; // 当前嵌套列表
    let i = start;
    for (; i < lines.length; i++) {
      const line = lines[i];
      if (!line.trim()) break;
      const m = RE_ITEM.exec(line);
      if (!m) break;
      // 有序/无序切换 → 换一个列表
      if (/^\d/.test(m[2]) !== (box.tagName === "OL")) break;
      const nested = m[1].length >= 2;
      const item = rich("li", "md-li", m[3]);
      if (nested && cur) {
        if (!sub) {
          sub = el(box.tagName === "OL" ? "ol" : "ul", "md-list md-list-sub");
          cur.appendChild(sub);
        }
        sub.appendChild(item);
      } else {
        cur = item;
        sub = null;
        box.appendChild(item);
      }
    }
    return { node: box, next: i };
  }

  /// 引用块: 连续的 `>` 行合成一个 `<blockquote>`, 行内语法照常解析。
  function quote(lines, start) {
    const box = el("blockquote", "md-quote");
    let i = start;
    for (; i < lines.length; i++) {
      const m = RE_QUOTE.exec(lines[i]);
      if (!m) break;
      box.appendChild(document.createTextNode("\n"));
      inline(box, m[1]);
    }
    return { node: box, next: i };
  }

  /// 表格: 要求**两行以上**且第二行是分隔行 —— 否则正文里的 `a | b` 会被误判成表格。
  function table(lines, start) {
    if (start + 1 >= lines.length || !RE_TSEP.test(lines[start + 1])) return null;
    const body = cells(lines[start]);
    const box = el("table", "md-table");
    const head = el("thead", "md-thead");
    const hrow = el("tr", "md-tr");
    for (const c of body) hrow.appendChild(rich("th", "md-th", c));
    head.appendChild(hrow);
    box.appendChild(head);
    const tb = el("tbody", "md-tbody");
    let i = start + 2;
    for (; i < lines.length; i++) {
      const line = lines[i];
      if (!line.trim() || !line.includes("|")) break;
      const tr = el("tr", "md-tr");
      const cs = cells(line);
      for (let k = 0; k < body.length; k++) tr.appendChild(rich("td", "md-td", cs[k] || ""));
      tb.appendChild(tr);
    }
    box.appendChild(tb);
    return { node: box, next: i };
  }

  /// 受限 Markdown → `DocumentFragment`。不认识的行按段落原样输出。
  function render(text) {
    const frag = document.createDocumentFragment();
    const lines = String(text === undefined || text === null ? "" : text).split("\n");
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];

      if (!line.trim()) {
        i++;
        continue;
      }

      const fence = RE_FENCE.exec(line);
      if (fence) {
        const lang = fence[1].trim();
        const body = [];
        i++;
        while (i < lines.length && !RE_FENCE.test(lines[i])) {
          body.push(lines[i]);
          i++;
        }
        i++; // 吃掉收尾的 ``` (没有就不动, 不报错)
        const pre = el("pre", "md-pre");
        const code = el("code", lang ? "md-code md-code-block" : "md-code-block", body.join("\n"));
        if (lang) code.dataset.lang = lang;
        pre.appendChild(code);
        frag.appendChild(pre);
        continue;
      }

      const head = RE_HEAD.exec(line);
      if (head) {
        const level = Math.min(head[1].length, 3); // 正文里 4-6 级与 3 级同观感, 不另设样式
        frag.appendChild(rich("div", "md-h md-h" + level, head[2]));
        i++;
        continue;
      }

      if (RE_HR.test(line)) {
        frag.appendChild(el("hr", "md-hr"));
        i++;
        continue;
      }

      if (RE_QUOTE.test(line)) {
        const r = quote(lines, i);
        frag.appendChild(r.node);
        i = r.next;
        continue;
      }

      if (RE_ITEM.test(line)) {
        const r = list(lines, i);
        frag.appendChild(r.node);
        i = r.next;
        continue;
      }

      if (line.includes("|")) {
        const r = table(lines, i);
        if (r) {
          frag.appendChild(r.node);
          i = r.next;
          continue;
        }
      }

      // 段落: 一直吃到空行或下一个块级开头。
      const para = [];
      while (i < lines.length && lines[i].trim() && !startsBlock(lines[i])) {
        para.push(lines[i]);
        i++;
      }
      if (!para.length) {
        // 兜底: 块级开头但没被上面任何分支接住(理论上到不了), 原样输出, 绝不吞行。
        frag.appendChild(el("div", "md-p", line));
        i++;
        continue;
      }
      const p = rich("div", "md-p", para.join("\n"));
      frag.appendChild(p);
    }
    return frag;
  }

  R.markdown = { render: render };
})();
