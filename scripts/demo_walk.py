#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
ricow Web UI「新建实盘」· demo 测试网真机走查 (可复跑回归脚本)
================================================================

为什么需要它
------------
`e2e_web.py` / `probe_web_ui.mjs` 能证明"那段字符串被服务出去了"、能抓浏览器运行期
异常, 但它们**不触碰真实下单路径**。demo 模式闭环里最关键的判据是:

    Web UI 选 demo → 启动 → 策略真的把一张订单发到了 demo 测试网交易所。

本脚本用真实浏览器(Playwright + 系统 Edge)走完整闭环, 并**直接用 demo 测试网密钥
签名查询 openOrders** 作为"订单确实到达交易所"的不可辩驳证据, 停机(勾选平仓)后再查一次
确认干净撤单。它只在**模拟盘(demo.binance.com)**上跑, 不碰任何真实资金。

前置条件(运行前准备好)
----------------------
1) 起一个隔离的验证 daemon(自带 root, 不污染主仓):
     RICOW_ROOT=<repo>/tmp/demo-walk-root ./target/debug/ricow web --no-open --port 18778
   root 的 `ricow.toml` 必须含:
     [exchange]
     demo_key    = "..."      # demo 测试网密钥(从你本机 ricow.toml 取, 不入仓)
     demo_secret = "..."
     show_all_pairs = true    # 回测/全量视野需要
2) 浏览器: 装好 Playwright + 系统 Edge(`playwright install msedge` 或复用已装)。
3) 环境变量(见下方 ENV)。

怎么跑
------
   # 密钥可二选一提供:
   #   a) 直接给:
   export RICOW_DEMO_KEY=... RICOW_DEMO_SECRET=...
   #   b) 不给则自动从 RICOW_DEMO_TOML(默认 <repo>/ricow.toml)的 [exchange] 读 demo_key/demo_secret
   RICOW_WEB_URL="http://127.0.0.1:18778/?token=<T>" \
   RICOW_ROOT="<repo>/tmp/demo-walk-root" \
   python scripts/demo_walk.py

脚本会自动在 <root>/strategies/spot/ 下写入一个"demo 走查探针"策略(首个 tick 真实提交
一张永不成交的限价买单), 无需手动准备策略。

ENV(均可用环境变量覆盖; 标 * 为必填)
   RICOW_WEB_URL*   已运行的 ricow web 地址(含 ?token=)
   RICOW_ROOT*      daemon 数据根(用于写策略 + 读 <root>/logs/<id>.log)
   RICOW_DEMO_KEY   demo 测试网 API key(缺则读 RICOW_DEMO_TOML)
   RICOW_DEMO_SECRET demo 测试网 API secret(缺则读 RICOW_DEMO_TOML)
   RICOW_DEMO_TOML  demo 密钥来源 toml(默认 <repo>/ricow.toml)
   RICOW_STRATEGY   探针策略 id(默认 demo_walk_probe)
   RICOW_PAIR       交易对(默认 ETHUSDT)
   DEMO_HOST        demo 现货 REST 基址(默认 https://demo-api.binance.com)
   RICOW_OUT        截图/账目输出目录(默认 <repo>/tmp/demo-walk)

退出码: 0 = 全绿; 1 = 有 FAIL(闭环未达成); 2 = 脚本自身异常(缺环境/缺依赖/连不上)。
"""
import sys, os, json, time, re, urllib.request, urllib.error, hmac, hashlib
from pathlib import Path

# ---- 依赖: Playwright(与 e2e_web.py 同 lineage) ----
try:
    from playwright.sync_api import sync_playwright
except Exception as e:  # noqa
    sys.stderr.write("缺少依赖 playwright: pip install playwright && playwright install msedge\n")
    sys.exit(2)

REPO = Path(__file__).resolve().parent.parent

def die(msg, code=2):
    sys.stderr.write(msg.rstrip("\n") + "\n")
    sys.exit(code)

def need_env(name, default=None):
    v = os.environ.get(name, default)
    if v is None or v == "":
        die(f"缺少环境变量 {name}（详见脚本头部说明）", 2)
    return v

# ---- 配置(ENV 优先, 否则默认值) ----
WEB_URL   = os.environ.get("RICOW_WEB_URL", "")
ROOT      = os.environ.get("RICOW_ROOT", "")
STRATEGY  = os.environ.get("RICOW_STRATEGY", "demo_walk_probe")
PAIR      = os.environ.get("RICOW_PAIR", "ETHUSDT")
DEMO_HOST = os.environ.get("DEMO_HOST", "https://demo-api.binance.com")
OUT       = Path(os.environ.get("RICOW_OUT", str(REPO / "tmp" / "demo-walk")))
# 启动重试次数: 容忍启动期 demo 网络抖动(时钟预检/exchangeInfo 偶发超时)。
MAX_START_ATTEMPTS = int(os.environ.get("RICOW_START_ATTEMPTS", "3"))

if not WEB_URL:
    die("缺少环境变量 RICOW_WEB_URL（已运行的 ricow web 地址, 含 ?token=）", 2)
if not ROOT:
    die("缺少环境变量 RICOW_ROOT（daemon 数据根）", 2)

# ---- demo 密钥: 显式 ENV 优先, 否则从 RICOW_DEMO_TOML 的 [exchange] 读 ----
DEMO_KEY = os.environ.get("RICOW_DEMO_KEY", "")
DEMO_SECRET = os.environ.get("RICOW_DEMO_SECRET", "")
if not DEMO_KEY or not DEMO_SECRET:
    toml_path = Path(os.environ.get("RICOW_DEMO_TOML", str(REPO / "ricow.toml")))
    if not toml_path.exists():
        die(f"未提供 RICOW_DEMO_KEY/SECRET, 且找不到密钥源 {toml_path}", 2)
    txt = toml_path.read_text(encoding="utf-8", errors="replace")
    # 只在 [exchange] 段内抓 demo_key / demo_secret
    seg = ""
    in_ex = False
    for line in txt.splitlines():
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            in_ex = (s == "[exchange]")
            continue
        if in_ex:
            seg += line + "\n"
    m = re.search(r'demo_key\s*=\s*"([^"]+)"', seg)
    if m: DEMO_KEY = m.group(1)
    m = re.search(r'demo_secret\s*=\s*"([^"]+)"', seg)
    if m: DEMO_SECRET = m.group(1)
    if not DEMO_KEY or not DEMO_SECRET:
        die(f"在 {toml_path} 的 [exchange] 段未找到 demo_key/demo_secret", 2)

TOKEN = ""
m = re.search(r"[?&]token=([^&]+)", WEB_URL)
if m: TOKEN = m.group(1)

console_errors = []
page_errors = []
bad_responses = []  # 非 2xx 响应的 "状态码 URL" —— 让 console 里的 404/500 可定位到具体端点

def log(*a):
    print(*a, flush=True)

_ANSI = re.compile(r"\x1b\[[0-9;]*m")  # 剥掉引擎日志里的 ANSI 颜色转义码(否则会插在 "k=v" 之间, 正则匹配不到)

def read_disk_log():
    p = Path(ROOT) / "logs" / f"{STRATEGY}.log"
    try:
        return _ANSI.sub("", p.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        return ""

def demo_open_orders(symbol):
    """对 demo 测试网发起签名查询, 返回该 symbol 的当前挂单列表 —— 订单确实到达交易所的铁证。带重试。
    注意1: 必须带 X-MBX-APIKEY 头, 否则签名查询被拒(400)。
    注意2: 签名 URL **不能跨重试复用** —— timestamp 会过期, 币安返回 -1021
           "Timestamp for this request is outside of the recvWindow"(HTTP 400)。
           所以每次重试都要重新取时间戳并重新签名。"""
    def build_url():
        params = {"symbol": symbol, "timestamp": int(time.time() * 1000), "recvWindow": 10000}
        qs = "&".join(f"{k}={v}" for k, v in sorted(params.items()))
        sig = hmac.new(DEMO_SECRET.encode(), qs.encode(), hashlib.sha256).hexdigest()
        return f"{DEMO_HOST}/api/v3/openOrders?{qs}&signature={sig}"
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))  # 绕系统代理直连
    return http_get_json(build_url, timeout=20, opener=opener,
                         headers={"X-MBX-APIKEY": DEMO_KEY})

def http_get_json(url, timeout=20, opener=None, headers=None):
    """直连(绕系统代理) GET 并解析 JSON, 对瞬时超时/连接抖动做有限重试。
    demo 测试网网络本就间歇, 单次失败不代表环境整体不可用。
    url 可以是字符串, 也可以是**无参可调用对象**(每次调用返回一个新 URL)——
    签名请求必须用后者, 否则重试会复用过期 timestamp 被判 -1021。"""
    if opener is None:
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    last = None
    for attempt in range(4):
        try:
            u = url() if callable(url) else url
            req = urllib.request.Request(u, headers=headers or {})
            with opener.open(req, timeout=timeout) as r:
                return json.loads(r.read().decode())
        except urllib.error.HTTPError as e:
            # 带上响应体: 币安的错误码/消息只在 body 里(如 -1021), 只看状态码等于没取证
            try:
                body = e.read().decode()[:300]
            except Exception:
                body = ""
            last = f"HTTP {e.code} {body}"
            if attempt < 3:
                time.sleep(3)
        except Exception as e:  # noqa
            last = f"{type(e).__name__}: {e}"
            if attempt < 3:
                time.sleep(3)
    return {"_error": str(last)[:400]}

def preflight_demo():
    """启动前探活 demo 测试网 REST。不可达 → 明确是环境故障, exit 2, 不要误导成产品 FAIL。
    注意: /api/v3/time 公开端点不接受任何参数(多带会 -1101), 所以这里裸打。带重试抵御间歇抖动。"""
    res = http_get_json(f"{DEMO_HOST}/api/v3/time")
    if isinstance(res, dict) and "_error" in res:
        log(f"预检: demo 测试网 REST 不可达 ({res['_error']}) —— 这是环境故障, 非产品问题")
        return False
    log("预检: demo 测试网 REST 可达 ✓")
    return True

def ensure_no_running_instance(page):
    """幂等: 若已有同策略实例在运行(上轮 abort 遗留), 先停掉, 避免重复/干扰。"""
    page.evaluate("if(window.R && R.navigate){ R.navigate('#runs'); } else { location.hash='#runs'; }")
    page.wait_for_timeout(1500)
    try:
        row = page.locator(f"tr:has-text('{STRATEGY}')").first
        if row.count() == 0:
            return
        mini = row.locator("button.sg-btn-mini").first
        if mini.count() == 0:
            return
        # 仅当确实是"运行中"才停(按钮文案为"停止"); 已停止的行不点。
        txt = mini.inner_text()
        if "停止" in (txt or ""):
            log("清理: 发现遗留的运行实例, 先停止")
            mini.click()
            page.wait_for_selector(".sg-modal-actions .sg-btn-primary", timeout=10000)
            page.locator(".sg-modal-actions .sg-btn-primary").click()
            page.wait_for_timeout(4000)
    except Exception as e:  # noqa
        log("清理: 检查遗留实例时异常(忽略):", e)

# ---- 自举: 写入"demo 走查探针"策略(首个 tick 真实提交一张永不成交的限价买单) ----
LUA = f'''-- 自动生成 by scripts/demo_walk.py —— demo 测试网真机走查探针, 不进主仓。
-- 目的: 在首个 tick 向 demo 测试网(demo-api.binance.com 模拟盘)真实提交一张限价买单,
-- 用于闭环验证 Web UI「新建实盘」的 demo 模式确实把订单发到了交易所。
-- 限价 = 买一价下方 20% (落在 PERCENT_PRICE_BY_SIDE [0.5,1.2] 带内, 永不成交),
-- 名义约 15 USDT (高于最小名义 5)。停机(勾选平仓)时引擎撤掉该挂单, 不留持仓。
-- 仅在每个运行实例的首个有效 tick 提交一次; 之后返回空表, 避免重复挂单。

local placed = false

function on_tick(ctx)
  if placed then
    return {{}}
  end

  local bid = ctx:best_bid("{PAIR}")
  if bid == nil then
    ctx:log("DEMO verify: 盘口尚未就绪, 等待下一 tick")
    return {{}}
  end

  -- 买一价下方 20%, 取整到 tick 0.01; 名义 >= 5 USDT。
  local limit = math.floor(bid * 0.8 * 100) / 100
  local size = 0.01
  placed = true
  ctx:log(string.format(
    "DEMO verify: 提交限价买单 price=%.2f size=%.4f (demo 模拟盘, 无真实资金)",
    limit, size))
  return {{
    {{ pair = "{PAIR}", side = "buy", size = size, price = limit, order_type = "limit" }},
  }}
end
'''

MANIFEST = f'''id = "{STRATEGY}"
name = "demo 走查探针(自动生成)"
market = "spot"
summary = "demo 测试网真机走查专用: 首个 tick 真实提交一张永不成交的限价买单, 用于验证 Web UI 新建实盘 demo 模式确实向交易所下单。"
description = "自动生成, 仅供 scripts/demo_walk.py 使用。"

[[params]]
key = "pair"
name = "交易对"
type = "string"
desc = "现货交易对"
required = true
'''

# 运行实例配置(根 strategies/<id>.toml): 引擎 start 路径要读它(script_path 指向 lua),
# 与 UI「save instance」写出的形状一致。缺它 → start 报"策略不存在"。
INSTANCE = f'''[strategy]
name = "{STRATEGY}"
type = "lua"
enabled = true
exchange = "binance"
live_enabled = false
market = "spot"
position_mode = "one-way"

[strategy.params]
pair = "{PAIR}"
script_path = "spot/{STRATEGY}.lua"
'''

def bootstrap_strategy():
    d = Path(ROOT) / "strategies" / "spot"
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{STRATEGY}.lua").write_text(LUA, encoding="utf-8")
    (d / f"{STRATEGY}.toml").write_text(MANIFEST, encoding="utf-8")
    # 运行实例配置(根目录): 引擎 start 路径依赖它
    (Path(ROOT) / "strategies" / f"{STRATEGY}.toml").write_text(INSTANCE, encoding="utf-8")
    log(f"已写入探针策略: {d / STRATEGY}.{{lua,toml}} + 实例配置 strategies/{STRATEGY}.toml")

# ---- 走查主流程 ----
fails = 0
def check(name, cond, detail=""):
    global fails
    if cond:
        log(f"  PASS  {name}")
    else:
        fails += 1
        log(f"  FAIL  {name}   <-- {detail}")

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    if not preflight_demo():
        log("环境预检失败: demo 测试网当前不可达。这是网络/环境问题, 不是产品缺陷。")
        log("请稍后网络恢复再跑; 退出码 2 表示环境故障。")
        sys.exit(2)
    bootstrap_strategy()

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True, channel="msedge")
        page = browser.new_page(viewport={"width": 1400, "height": 1000})
        page.on("console", lambda m: console_errors.append(m.text) if m.type == "error" else None)
        page.on("pageerror", lambda e: page_errors.append(str(e)))
        page.on("response", lambda r: bad_responses.append(f"{r.status} {r.url}") if r.status >= 400 else None)

        log("== 0) 幂等清理: 停掉可能遗留的同策略运行实例 ==")
        ensure_no_running_instance(page)

        log("== 1) 进入 #runs 视图 ==")
        page.goto(WEB_URL, wait_until="domcontentloaded")
        page.wait_for_timeout(1500)
        page.evaluate("if(window.R && R.navigate){ R.navigate('#runs'); } else { location.hash='#runs'; }")
        page.wait_for_timeout(1500)
        page.wait_for_selector("#rn-new-live", timeout=15000)
        page.screenshot(path=str(OUT / "01_runs.png"))
        log("runs 视图已加载")

        log("== 2) 打开「新建实盘」弹窗并搜索探针策略 ==")
        page.click("#rn-new-live")
        page.wait_for_selector(".sg-modal-card", timeout=10000)
        page.wait_for_selector(".rn-strat-search input[type=search]", timeout=10000)
        page.fill(".rn-strat-search input[type=search]", STRATEGY)
        page.wait_for_selector(".rn-strat-results .rn-strat-item", timeout=10000)
        page.locator(".rn-strat-results .rn-strat-item").first.click()
        log("策略已选中")

        log("== 3) 等待参数表单, 设置交易对 ==")
        page.wait_for_function(
            "() => { const b=document.querySelector('.rn-live-params'); return b && !b.hidden && b.children.length>1; }",
            timeout=15000)
        pair = page.locator("label.sg-field", has_text="交易对").locator("input").first
        cur = pair.input_value()
        if (cur or "").strip().upper() != PAIR.upper():
            pair.fill(PAIR)
        page.screenshot(path=str(OUT / "02_modal.png"))

        log("== 4) 选择 demo 模式, 校验启动钮可用 ==")
        page.locator("input[name='rn-start-mode'][value='demo']").click()
        page.wait_for_timeout(800)
        info = page.evaluate("""() => {
            const hint = document.querySelector('.sg-msg.err');
            const startBtn = document.querySelector('.sg-modal-actions .sg-btn-primary');
            return {
                demoHintHidden: hint ? hint.hidden : null,
                demoHintText: hint ? hint.textContent : null,
                startDisabled: startBtn ? startBtn.disabled : null,
                startText: startBtn ? startBtn.textContent : null,
            };
        }""")
        log("demo 弹窗状态:", json.dumps(info, ensure_ascii=False))
        page.screenshot(path=str(OUT / "02b_modal_demo.png"))
        check("demo 模式启动钮可用", not info.get("startDisabled"),
              f"demoHint={info.get('demoHintText')}")
        if info.get("startDisabled"):
            browser.close()
            sys.exit(1)

        log("== 5) 点击启动(demo) [最多重试 %d 次, 容忍启动期 demo 网络抖动] ==" % MAX_START_ATTEMPTS)
        # 记录启动前日志长度, step 7/step 11 只扫描"本轮新增"内容,
        # 避免跨运行累积的日志里遗留的旧提交标记造成假绿/假红。
        pre_len = len(read_disk_log())
        start_btn = page.locator(".sg-modal-actions .sg-btn-primary")
        running_ok = False
        for attempt in range(1, MAX_START_ATTEMPTS + 1):
            log(f"-- 启动尝试 {attempt}/{MAX_START_ATTEMPTS} --")
            # 弹窗开(首轮或失败重试后仍在)才点; 若已关(上轮已成功启动)则不点, 直接进入等待。
            if start_btn.count() and start_btn.first.is_visible():
                page.wait_for_timeout(500)
                start_btn.first.click()
                page.wait_for_timeout(3000)
            page.screenshot(path=str(OUT / f"03_after_start_{attempt}.png"))
            log("== 6) 等待 demo 运行行出现并进入运行中 ==")
            deadline = time.time() + 90
            while time.time() < deadline:
                row = page.evaluate("""() => {
                    const trs = Array.from(document.querySelectorAll('tr'));
                    const tr = trs.find(t => t.textContent.includes('%s'));
                    if (!tr) return null;
                    const mode = tr.querySelector('.rn-mode-demo') ? 'demo' : (tr.querySelector('.sg-badge')?.textContent || '');
                    const running = !!tr.querySelector('.rn-dot-run') || tr.textContent.includes('运行中');
                    return { found:true, mode, running };
                }""" % STRATEGY)
                if row and row.get("mode") == "demo":
                    if row.get("running"):
                        running_ok = True
                        break
                time.sleep(3)
            if running_ok:
                break
            log(f"启动尝试 {attempt} 未在 90s 内进入运行中(多半是 demo 网络抖动); 等待后重试")
            time.sleep(5)
        page.screenshot(path=str(OUT / "04_running.png"))
        # 硬失败: 多次启动尝试后仍没进入运行中, 多半是 demo 网络持续故障(如 exchangeInfo/时钟预检超时),
        # 继续跑后续步骤只会得到一堆误导性 FAIL。明确报错后退出。
        if not running_ok:
            check("demo 运行行进入运行中", False,
                  f"{MAX_START_ATTEMPTS} 次启动尝试均未进入运行中(可能是 demo 网络持续抖动导致启动失败, 非产品缺陷)")
            log("demo 运行未能进入运行中, 终止(退出码 1)。如为网络问题请重试。")
            browser.close()
            sys.exit(1)
        check("demo 运行行进入运行中", True)

        log("== 7) 等待策略真实向 demo 测试网提交订单 ==")
        submit_marker = "DEMO verify: 提交限价买单"
        order_confirmed = False
        poll_deadline = time.time() + 90
        while time.time() < poll_deadline:
            txt = read_disk_log()[pre_len:]   # 只看本轮新增
            if submit_marker in txt:
                idx = txt.index(submit_marker)
                tail = txt[idx:]
                if "下单失败" in tail[:800]:
                    log("FAIL: 订单被拒/失败:", tail[:800])
                    break
                order_confirmed = True
                log("策略已产单并提交(本轮日志标记命中)")
                break
            time.sleep(3)
        if not order_confirmed:
            log("WARN: 90s 内磁盘标记未命中, 改由 openOrders 直接核验")

        log("== 8) 直接签名查询 demo 测试网 openOrders(铁证) ==")
        time.sleep(5)
        oo = demo_open_orders(PAIR)
        log("demo openOrders:", json.dumps(oo, ensure_ascii=False))
        open_before = len(oo) if isinstance(oo, list) else 0
        our_orders = [o for o in oo if isinstance(o, dict) and o.get("symbol") == PAIR] if isinstance(oo, list) else []
        check("订单确实到达 demo 测试网(openOrders 含本 symbol 挂单)", len(our_orders) >= 1,
              f"openOrders={oo}")
        if our_orders:
            log("  命中挂单:", json.dumps(our_orders[0], ensure_ascii=False)[:300])

        # 引擎侧订单日志
        log("== 引擎侧订单日志 ==")
        for line in read_disk_log().splitlines():
            l = line.lower()
            if any(k in l for k in ["demo verify", "提交", "下单", "submitted", "order", "new", "cancel"]):
                log("  >", line.strip()[:220])

        log("== 9) 勾选平仓停止 ==")
        stop_btn = page.locator(f"tr:has-text('{STRATEGY}') button.sg-btn-mini").first
        stop_btn.click()
        page.wait_for_selector(".sg-modal-actions .sg-btn-primary", timeout=10000)
        page.wait_for_selector(".sg-modal-card", timeout=10000)
        try:
            cb = page.locator(".rn-stop-check input[type=checkbox]").first
            if not cb.is_checked():
                cb.check()
            log("停止: 已勾选平仓")
        except Exception as e:  # noqa
            log("停止: 无平仓勾选项(ok):", e)
        page.screenshot(path=str(OUT / "05_stop_modal.png"))
        page.locator(".sg-modal-actions .sg-btn-primary").click()
        page.wait_for_timeout(5000)
        page.screenshot(path=str(OUT / "06_stopped.png"))

        log("== 10) 停机后二次查询 openOrders(应为空=已撤) ==")
        time.sleep(3)
        oo_after = demo_open_orders(PAIR)
        log("demo openOrders AFTER stop:", json.dumps(oo_after, ensure_ascii=False))
        open_after = len(oo_after) if isinstance(oo_after, list) else 0
        check("停机后挂单已干净撤销(openOrders 为空)", open_after == 0, f"openOrders={oo_after}")

        log("== 11) 解析引擎最终账目 ==")
        txt = read_disk_log()[pre_len:]   # 只看本轮新增
        (OUT / "demo_log_disk.txt").write_text(txt, encoding="utf-8")
        submitted = canceled = order_failed = None
        for line in txt.splitlines():
            if "会话订单账目" in line:
                m = re.search(r"submitted=(\d+)", line);  submitted = int(m.group(1)) if m else submitted
                m = re.search(r"canceled=(\d+)", line);   canceled  = int(m.group(1)) if m else canceled
            if "统计:" in line:
                m = re.search(r"下单失败=(\d+)", line);    order_failed = int(m.group(1)) if m else order_failed
        log(f"  解析(本轮): submitted={submitted} canceled={canceled} 下单失败={order_failed}")
        check("引擎账目 submitted >= 1", submitted is not None and submitted >= 1, f"submitted={submitted}")
        check("引擎账目 canceled >= 1", canceled is not None and canceled >= 1, f"canceled={canceled}")
        check("引擎账目 下单失败 == 0", order_failed == 0, f"下单失败={order_failed}")
        for line in txt.splitlines():
            if "会话订单账目" in line or "统计:" in line or "停机清理" in line:
                log("  >", line.strip()[:220])

        log("== 12) 浏览器运行期异常 ==")
        # 浏览器对未声明 favicon 自动请求 /favicon.ico, 本服务所有端点都在 token 门后必然 404,
        # 属浏览器固定行为而非产品路径; 单独识别, 不计入 FAIL。
        real_page_errors = [e for e in page_errors if "favicon" not in e.lower()]
        real_console_errors = [
            e for e in console_errors
            if "favicon" not in e.lower() and "404" not in e and "/favicon.ico" not in e
        ]
        log(f"  page_errors={len(page_errors)} (favicon 噪声 {len(page_errors)-len(real_page_errors)}), "
            f"console_errors={len(console_errors)} (非 favicon {len(real_console_errors)})")
        for e in console_errors[:10]:
            log("    CONSOLE>", e[:200])
        # 非 2xx 响应按 URL 定位: console 文本只说"404/500", 定位不到端点就等于没取证
        real_bad = [b for b in bad_responses if "favicon" not in b.lower()]
        for b in real_bad[:15]:
            log("    HTTP>", b[:200])
        check("零浏览器运行期异常(不含 favicon/404)", len(real_page_errors) == 0 and len(real_console_errors) == 0,
              "; ".join((real_page_errors + real_console_errors)[:5]))
        # 端点级非 2xx: 500 一律算产品侧问题(不是浏览器噪声); 404 只在非 favicon 时提示
        bad5xx = [b for b in real_bad if b.startswith("5")]
        check("无 5xx 端点响应", len(bad5xx) == 0, "; ".join(bad5xx[:5]))

        browser.close()

    log(f"\n==== 结果: {'全绿 ✅' if fails == 0 else str(fails) + ' 项 FAIL ❌'} ====")
    log(f"截图/账目: {OUT}")
    sys.exit(0 if fails == 0 else 1)

if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa
        import traceback
        traceback.print_exc()
        die(f"脚本异常: {e}", 2)
