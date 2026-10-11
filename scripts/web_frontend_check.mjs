// Ricow Web 前端静态检查 (D13) —— **零依赖、可选 job**。
//
// 为什么单独一个脚本, 不塞进 `cargo test`:
//   它要读 `web/assets/*.js` 的**源码文本**做一致性分析, 用 Rust 写等于把 JS 语法解析一遍;
//   而引入 node 到 `cargo test` 就把 CI 多绑一个运行时 (与"单二进制、零供应链"的取向相悖)。
//   所以它是**可选 job**: 本机 / 前端改动时跑一次, 不参与 `cargo test`。
//
// 用法:
//   node scripts/web_frontend_check.mjs        # 退出码 = 失败条数 (0 = 全绿)
//
// 它盯的是两类"不会让任何测试变红"的问题:
//   ① **双语字典缺口**: `t("xxx")` 里的键只在 `R.TEXT.zh` 有、`en` 没有(或反之) ——
//      运行时静默渲染 `undefined`, 页面上一块空白, 谁也看不出哪里错。
//   ② **路由解析的边界**: `parseHash` 是纯函数, 却一直只被"真机点一遍"覆盖; 这里直接喂表。
//
// 不做什么: 不跑浏览器、不装依赖、不解析完整 JS —— 语法合法性用 `vm.Script` 顺带粗验。
// 真正的行为验证仍在 `e2e_web.py`(它需要真服务 + 真浏览器语义)。

import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ASSETS = path.join(ROOT, "crates", "ricow", "src", "web", "assets");

let failed = 0;
const pass = (msg, extra = "") => console.log(`[ok] ${msg}${extra ? "  " + extra : ""}`);
const fail = (msg, detail) => {
  failed += 1;
  console.log(`[红线] ${msg}`);
  for (const line of String(detail).split("\n").slice(0, 20)) console.log(`    ${line}`);
};

// ---------------------------------------------------------------------------
// 工具: 从源码里抠出 `R.TEXT.<lang>` 字典的键。
//
// 不能直接对整块正则 —— 值里可能有 `:` / `{` / `}` (中文冒号不用管, 但 `"…: …"` 会)。
// 所以先按**跳过字符串字面量**的方式配平花括号切出块, 再把字符串替换成 `""` 后取键。
// ---------------------------------------------------------------------------
function sliceBraced(src, openIdx) {
  let depth = 0;
  let inStr = null;
  for (let i = openIdx; i < src.length; i++) {
    const ch = src[i];
    if (inStr) {
      if (ch === "\\") i += 1;
      else if (ch === inStr) inStr = null;
      continue;
    }
    if (ch === '"' || ch === "'" || ch === "`") inStr = ch;
    else if (ch === "{") depth += 1;
    else if (ch === "}") {
      depth -= 1;
      if (depth === 0) return src.slice(openIdx, i + 1);
    }
  }
  return null;
}

function blankStrings(block) {
  let out = "";
  let inStr = null;
  for (let i = 0; i < block.length; i++) {
    const ch = block[i];
    if (inStr) {
      if (ch === "\\") i += 1;
      else if (ch === inStr) {
        inStr = null;
        out += '""';
      }
      continue;
    }
    if (ch === '"' || ch === "'" || ch === "`") inStr = ch;
    else out += ch;
  }
  return out;
}

/// 某个源码文件里 `R.TEXT.<lang>` 的键(按出现顺序)。两种写法都要认:
///   ① 模块写法: `Object.assign(R.TEXT.zh, { … })` / `R.TEXT.zh = { … }`;
///   ② 公共件的基础字典: `R.TEXT = { zh: { … }, en: { … } }` —— 这里语言名是**对象键**,
///      不是属性访问 (`R.TEXT.zh` 一个都匹配不到)。只认 ① 的后果很隐蔽: 基础字典那批键
///      全被当成"没有定义", C 段一次报几十条假红线。
function textKeys(src, lang) {
  const keys = [];
  // ① 语言是属性名。
  const re = new RegExp(`R\\.TEXT\\.${lang}\\s*[,=]?\\s*\\{`, "g");
  let m;
  while ((m = re.exec(src)) !== null) {
    const block = sliceBraced(src, m.index + m[0].length - 1);
    if (block) keys.push(...keysOfBlock(block));
  }
  // ② 语言是对象键(基础字典)。
  const baseRe = /R\.TEXT\s*=\s*\{/g;
  let b;
  while ((b = baseRe.exec(src)) !== null) {
    const base = sliceBraced(src, b.index + b[0].length - 1);
    if (!base) continue;
    const subRe = new RegExp(`(?:^|[,{\\s])${lang}\\s*:\\s*\\{`, "g");
    let s;
    while ((s = subRe.exec(base)) !== null) {
      const sub = sliceBraced(base, base.indexOf("{", s.index));
      if (sub) keys.push(...keysOfBlock(sub));
    }
  }
  return keys;
}

function keysOfBlock(block) {
  return [...blankStrings(block).matchAll(/([A-Za-z_$][\w$]*)\s*:/g)].map((m) => m[1]);
}

// ---------------------------------------------------------------------------
// 读入全部资产。
// ---------------------------------------------------------------------------
const jsFiles = fs.readdirSync(ASSETS).filter((f) => f.endsWith(".js")).sort();
const html = fs.readFileSync(path.join(ASSETS, "index.html"), "utf8");
const sources = new Map(jsFiles.map((f) => [f, fs.readFileSync(path.join(ASSETS, f), "utf8")]));

console.log("=".repeat(78));
console.log("A. 语法 (每个 .js 交给 V8 编译一次, 等价于 `node --check`)");
console.log("=".repeat(78));
for (const [name, src] of sources) {
  try {
    new vm.Script(src, { filename: name });
  } catch (err) {
    fail(`${name} 语法错误`, String(err && err.message));
  }
}
pass(`${jsFiles.length} 份 .js 语法可编译`);

// ---------------------------------------------------------------------------
// B. 双语字典: zh / en 键集合必须一致。
// ---------------------------------------------------------------------------
console.log("");
console.log("=".repeat(78));
console.log("B. 双语文案字典 (R.TEXT.zh vs R.TEXT.en)");
console.log("=".repeat(78));
const zhAll = new Map();
const enAll = new Map();
for (const [name, src] of sources) {
  for (const k of textKeys(src, "zh")) zhAll.set(k, name);
  for (const k of textKeys(src, "en")) enAll.set(k, name);
}
if (zhAll.size === 0) {
  fail("没解析到任何双语键", "字典写法可能变了(本脚本按 `R.TEXT.zh = {…}` / Object.assign 取键)");
} else {
  const missingEn = [...zhAll.keys()].filter((k) => !enAll.has(k));
  const missingZh = [...enAll.keys()].filter((k) => !zhAll.has(k));
  if (missingEn.length) {
    fail(
      `${missingEn.length} 个键缺英文 (运行时静默渲染 undefined)`,
      missingEn.map((k) => `${k}  (定义于 ${zhAll.get(k)})`).join("\n")
    );
  }
  if (missingZh.length) {
    fail(
      `${missingZh.length} 个键缺中文`,
      missingZh.map((k) => `${k}  (定义于 ${enAll.get(k)})`).join("\n")
    );
  }
  if (!missingEn.length && !missingZh.length) {
    pass(`双语键一一对应`, `(zh ${zhAll.size} · en ${enAll.size})`);
  }
}

// ---------------------------------------------------------------------------
// C. 引用完整性: `t("literal")` / `R.TEXT.zh.xxx` 都必须有定义; 反向也查"定义了但没用"。
// ---------------------------------------------------------------------------
console.log("");
console.log("=".repeat(78));
console.log("C. 文案引用 (t(\"key\") / R.TEXT.*.key / 反向死文案)");
console.log("=".repeat(78));

// 051 复核 C3: 早先只认**双引号** `t("key")`, 单引号 / 模板串 / 变量形全在盲区 ——
// 一旦有人用单引号写新文案, 缺键就会**静默通过**。这里三种字面量形态都收。
const CALL_FORMS = [
  /\bt\(\s*"([A-Za-z_$][\w$]*)"\s*\)/g,
  /\bt\(\s*'([A-Za-z_$][\w$]*)'\s*\)/g,
  /\bt\(\s*`([A-Za-z_$][\w$]*)`\s*\)/g,
];
const used = new Map(); // key → 第一个引用它的文件
for (const [name, src] of sources) {
  for (const re of CALL_FORMS) {
    for (const m of src.matchAll(re)) {
      if (!used.has(m[1])) used.set(m[1], name);
    }
  }
  for (const m of src.matchAll(/R\.TEXT\.(?:zh|en)\.([A-Za-z_$][\w$]*)/g)) {
    if (!used.has(m[1])) used.set(m[1], name);
  }
}
const dangling = [...used.keys()].filter((k) => !zhAll.has(k) || !enAll.has(k));
if (dangling.length) {
  fail(
    `${dangling.length} 个文案键没有定义`,
    dangling.map((k) => `t("${k}") 于 ${used.get(k)}`).join("\n")
  );
} else {
  pass(`静态引用的 ${used.size} 个文案键都有中英定义`);
}

// 反向: 定义了但**在任何地方都没被引用**的键 = 死文案(切了语言也永远不显示)。
// 判据刻意保守, 只把"除了自己的定义块之外, 整个源码里连这个名字都不出现"的键判为死 ——
// 动态取值(`R.TEXT[lang][someVar]`)拿不到静态引用, 故不据此判死(避免假红线)。
const wholeWordCount = (src, key) => {
  const re = new RegExp(`\\b${key}\\b`, "g");
  return (src.match(re) || []).length;
};
const defCounts = new Map();
const allSrc = [...sources.values()].join("\n");
for (const [name, src] of sources) {
  for (const lang of ["zh", "en"]) {
    const re = new RegExp(`R\\.TEXT\\.${lang}\\s*[,=]?\\s*\\{`, "g");
    let m;
    while ((m = re.exec(src)) !== null) {
      const block = sliceBraced(src, m.index + m[0].length - 1);
      if (!block) continue;
      for (const k of keysOfBlock(block)) defCounts.set(k, (defCounts.get(k) || 0) + 1);
    }
  }
}
const dead = [...zhAll.keys()].filter((k) => wholeWordCount(allSrc, k) <= (defCounts.get(k) || 0));
if (dead.length) {
  fail(
    `${dead.length} 个文案键定义了却从未被引用(死文案)`,
    dead.map((k) => `${k}  (定义于 ${zhAll.get(k)})`).join("\n")
  );
} else {
  pass("没有「定义了但没人用」的死文案");
}

// ---------------------------------------------------------------------------
// D. 首页静态标签: `data-zh` / `data-en` 必须**逐标签**成对。
// ---------------------------------------------------------------------------
console.log("");
console.log("=".repeat(78));
console.log("D. 首页静态标签 (data-zh / data-en, 逐标签配对)");
console.log("=".repeat(78));

// 051 复核 C2: 早先只比 `data-zh` 与 `data-en` 的**总数量** —— 一个只带 data-zh 的标签
// 配另一个只带 data-en 的标签能"凑够数量"而通过, 但运行时前者在英文下被清成空字符串。
// 现在**按标签切片**比对: 含 `data-zh` 的标签必须同时含 `data-en`(placeholder 同理)。
const attrPresent = (tag, attr) => new RegExp(`\\s${attr}\\s*=`).test(tag);
const unpaired = [];
let zhTags = 0;
let enTags = 0;
let phZhTags = 0;
let phEnTags = 0;
for (const m of html.matchAll(/<[a-zA-Z][^>]*>/g)) {
  const tag = m[0];
  const z = attrPresent(tag, "data-zh");
  const e = attrPresent(tag, "data-en");
  const pz = attrPresent(tag, "data-zh-placeholder");
  const pe = attrPresent(tag, "data-en-placeholder");
  if (z) zhTags += 1;
  if (e) enTags += 1;
  if (pz) phZhTags += 1;
  if (pe) phEnTags += 1;
  if (z !== e) unpaired.push(`data-zh/en 不成对: ${tag.slice(0, 140)}`);
  if (pz !== pe) unpaired.push(`data-zh/en-placeholder 不成对: ${tag.slice(0, 140)}`);
}
if (unpaired.length) {
  fail(`${unpaired.length} 处静态标签双语属性不成对`, unpaired.join("\n"));
} else {
  pass(`首页 ${zhTags} 个标签逐标签成对 (含 ${phZhTags} 组 placeholder)`);
}

// ---------------------------------------------------------------------------
// E. 纯函数: router 的 hash 解析 (喂表, 不依赖浏览器)。
// ---------------------------------------------------------------------------
console.log("");
console.log("=".repeat(78));
console.log("E. 纯逻辑: router.parseHash");
console.log("=".repeat(78));
const routerSrc = sources.get("router.js");
let parseHash = null;
{
  const windowStub = {
    addEventListener() {},
    dispatchEvent() {},
  };
  const sandbox = {
    window: windowStub,
    document: { querySelectorAll: () => [], querySelector: () => null },
    location: { hash: "" },
    history: { replaceState() {} },
    console,
  };
  windowStub.Ricow = {};
  vm.createContext(sandbox);
  try {
    vm.runInContext(routerSrc, sandbox, { filename: "router.js" });
    parseHash = windowStub.Ricow.parseHash;
  } catch (err) {
    fail("router.js 在最小桩里跑不起来", String(err && err.message));
  }
}
if (parseHash) {
  // 表: [hash, 期望 name, 期望 param, 期望 invalid]
  const cases = [
    ["", "chat", null, false],
    ["#chat", "chat", null, false],
    ["#markets", "markets", null, false],
    ["#markets/BTCUSDT", "markets", "BTCUSDT", false],
    ["#markets/BTC%2FUSDT", "markets", "BTC/USDT", false], // 二级段解码一次
    ["#strategies/shannon_grid", "strategies", "shannon_grid", false],
    ["#markets/a/b", "markets", "a/b", false], // 多余斜杠合回 param
    ["#nope", "chat", null, true], // 未知视图 → 回退 chat 且标记 invalid
    ["#%E4%B8%AD", "chat", null, true], // 未知(非 ASCII)一级段同样回退
    ["#settings", "settings", null, false],
    ["#runs", "runs", null, false],
    ["#keys", "keys", null, false],
  ];
  const bad = [];
  for (const [hash, name, param, invalid] of cases) {
    const got = parseHash(hash);
    if (got.name !== name || got.param !== param || got.invalid !== invalid) {
      bad.push(
        `${JSON.stringify(hash)} → ${JSON.stringify(got)}; 期望 {name:${name}, param:${JSON.stringify(param)}, invalid:${invalid}}`
      );
    }
  }
  if (bad.length) fail(`parseHash ${bad.length}/${cases.length} 条不符`, bad.join("\n"));
  else pass(`parseHash ${cases.length} 条边界全对 (空 hash / 未知视图 / 解码 / 多段)`);
}

// ---------------------------------------------------------------------------
// F. toast 错误口径: `R.toast(..., "err")` **只允许**出现在命令面板。
// ---------------------------------------------------------------------------
console.log("");
console.log("=".repeat(78));
console.log("F. toast 错误口径 (err toast 只准命令面板用)");
console.log("=".repeat(78));

// 051 复核 B4: `common.js` 的约定是"toast 只承载成功/中性, 错误留在原地", 唯一例外是
// 命令面板(点完即关的一次性浮层, 动作失败时没有原位可贴)。这条例外必须有护栏 —— 否则
// 下一个人会照着 palette.js 抄, 例外悄悄变成常态。这里把"允许名单"钉死。
const ERR_TOAST_ALLOWED = new Set(["palette.js"]);
const errToastFiles = [];
for (const [name, src] of sources) {
  if (/R\.toast\([^;]*?"err"\s*\)/s.test(src)) errToastFiles.push(name);
}
const illegal = errToastFiles.filter((f) => !ERR_TOAST_ALLOWED.has(f));
if (illegal.length) {
  fail(
    `${illegal.length} 个文件在错误路径上用了 err toast (违反 toast 口径)`,
    illegal
      .map((f) => `${f}: 错误应留在原地的 msg 区; 只有命令面板这一处例外(见 common.js 的 R.toast 注释)`)
      .join("\n")
  );
} else {
  pass(`err toast 仅出现在允许名单内 (${errToastFiles.join(", ") || "无"})`);
}

console.log("");
if (failed === 0) {
  console.log("前端静态检查: 全绿");
  process.exit(0);
}
console.log(`前端静态检查: ${failed} 条未通过 (见上)`);
process.exit(failed);
