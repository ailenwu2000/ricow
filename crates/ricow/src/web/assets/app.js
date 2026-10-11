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
    // 点了导航项就收起(窄屏下抽屉正盖着目的地), Esc 同样收起。
    const nav = document.getElementById("nav");
    if (nav) nav.addEventListener("click", () => setOpen(false));
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
    R.chat.init();
    R.startRouter();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    // 脚本本就放在 body 末尾, 正常走这里; 保留分支以防日后被挪到 <head>。
    start();
  }
})();
