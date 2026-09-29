// Ricow Web 前端入口 (032): 只负责按序启动 —— 对话视图初始化(语言/会话/面板全在 chat.js
// 的 boot 内, 顺序与拆分前逐字一致)+ router 首跳(按当前 hash 恢复, 无匹配回退 #chat)。
//
// 加载顺序(index.html 底部): lightweight-charts.js → common.js → router.js → chat.js → app.js。
// 后续阶段的 settings/markets/strategies/runs 视图文件会插在 chat.js 与本文件之间。
"use strict";

(function () {
  const R = window.Ricow;

  function start() {
    // 无论首屏落在哪个 hash, 对话视图都照常后台初始化(SSE / 面板轮询与拆分前同节拍);
    // init 幂等, router 激活 #chat 时再调一次也不会重复启动。
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
