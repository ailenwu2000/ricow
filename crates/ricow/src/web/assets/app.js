// Ricow Web 前端入口 (032): 只负责按序启动 —— 对话视图初始化(语言/会话/面板全在 chat.js
// 的 boot 内, 顺序与拆分前逐字一致)+ router 首跳(按当前 hash 恢复, 无匹配回退 #chat)。
//
// 加载顺序(index.html 底部): lightweight-charts.js → common.js → router.js → chat.js → app.js。
// 后续阶段的 settings/markets/strategies/runs 视图文件会插在 chat.js 与本文件之间。
"use strict";

(function () {
  const R = window.Ricow;

  // C12: 窄屏导航抽屉(≤900px)。侧栏在窄屏是"盖上来的一层", 由顶栏 ☰ 开合;
  // 宽屏该按钮被 CSS 隐藏, 这段逻辑即便照跑也无副作用(状态位只在 class 上)。
  // 断点值 900 与 style.css 的 `@media (max-width: 900px)` 是同一个 —— 改一处要改两处。
  function bindSideToggle() {
    const app = document.getElementById("app");
    const toggle = document.getElementById("side-toggle");
    const scrim = document.getElementById("side-scrim");
    if (!app || !toggle) return;
    const setOpen = (open) => {
      app.classList.toggle("side-open", open);
      toggle.setAttribute("aria-expanded", open ? "true" : "false");
    };
    toggle.addEventListener("click", () => {
      setOpen(!app.classList.contains("side-open"));
    });
    if (scrim) scrim.addEventListener("click", () => setOpen(false));
    // 051 复核 B7: 收起判据从"只认 `#nav`"放宽到**整个侧栏** —— `#session-list` 与 `#nav`
    // 是同在 `#sidebar` 里的**兄弟**, 窄屏下点会话原先不收起, 抽屉正盖着刚打开的会话。
    // 宽屏时 `side-open` 位在 CSS 里本就无效果, 放宽无副作用。
    const sidebar = document.getElementById("sidebar");
    if (sidebar) sidebar.addEventListener("click", () => setOpen(false));
    // 程序化导航(palette / help 抽屉 / 空状态 CTA / 路由回退)不一定经过侧栏点击 ——
    // 只要 hash 变了就收起。点击**当前视图**的链接不触发 hashchange, 由上面的侧栏点击兜住。
    window.addEventListener("hashchange", () => setOpen(false));
    document.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") setOpen(false);
    });
    // 回到桌面宽度时复位, 免得留一个"半开"的状态位影响后续判断。
    window.addEventListener("resize", () => {
      if (window.innerWidth > 900) setOpen(false);
    });
  }

  function start() {
    // 无论首屏落在哪个 hash, 对话视图都照常后台初始化(SSE / 面板轮询与拆分前同节拍);
    // init 幂等, router 激活 #chat 时再调一次也不会重复启动。
    bindSideToggle();
    // 062: 给左侧导航注入 fmz 形态的线性图标(由 `ui.js` 的 `R.ui.decorateNav` 负责)。
    // 放在 `bindSideToggle` 之后、路由启动之前 —— 导航项是静态的, 何时注入都行,
    // 但要在首次渲染前完成, 否则会看到"文字先出现、图标后补"的跳动。
    try {
      if (R.ui && R.ui.decorateNav) R.ui.decorateNav(document.getElementById("nav"));
      document.documentElement.dataset.navIcons = String(
        document.querySelectorAll("#nav .nav-ico").length
      );
    } catch (err) {
      console.error("[ricow] 导航图标注入失败(不影响导航本身):", err);
    }
    // 051 复核 B8: `chat.init()` 若抛错(资产没加载成功 / 符号缺失), 早先会连带把下面的
    // `startRouter()` 一起吞掉 —— 页面停在首屏、视图切不动、且**没有任何提示**。
    // chat 是"增强", 路由是"骨架": 前者失败不该让后者停摆。错误如实打印到控制台, 不静默。
    try {
      R.chat.init();
    } catch (err) {
      console.error("[ricow] chat.init() 失败, 路由仍继续启动:", err);
    }
    R.startRouter();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    // 脚本本就放在 body 末尾, 正常走这里; 保留分支以防日后被挪到 <head>。
    start();
  }
})();
