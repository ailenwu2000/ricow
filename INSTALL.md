# INSTALL — 预编译包用户从这里开始

> 你下载的是 **release 页面的预编译包**(不需要 Rust 工具链)。想在源码上构建, 请回
> [README_zh.md](README_zh.md) §「快速开始」。

## 1. 解包

```bash
tar -xJf ricow-x86_64-unknown-linux-gnu.tar.xz    # Linux
# Windows: 右键解压 ricow-x86_64-pc-windows-msvc.zip
```

解出来是一个**以压缩包名命名的嵌套目录**(cargo-dist 的标准布局, 不是打错了包):

```
ricow-x86_64-unknown-linux-gnu/
├── ricow            # 主程序 (Windows: ricow.exe)
├── LICENSE
├── README.md        # 完整文档
├── INSTALL.md       # 本文件
└── 启动-ricow-*.sh / .cmd / .command   # 双击入口
```

## 2. 把 ricow 放进 PATH (任选其一)

- **最省事**: 直接用解包目录里的启动脚本(`启动-ricow-AI助手.*` / `启动-ricow-Web.*`), 它们会自动找到同目录的二进制;
- **全局使用**: 把 `ricow`(Windows: `ricow.exe`)复制或移动到已在 `PATH` 里的目录
  (Linux/macOS 常用 `~/.local/bin` 或 `/usr/local/bin`; Windows 常用 `%LocalAppData%\Programs`);
- 或把解包目录本身加进 `PATH`。

改完 `PATH` 后请**新开一个终端**再执行 `ricow --version` 验证 —— 当前终端不会自动刷新 `PATH`。

## 3. 第一次运行

```bash
./启动-ricow-AI助手.sh        # Linux/macOS
:: Windows 双击 启动-ricow-AI助手.cmd
```

- 首次启动会在**对话里**引导你选 AI 供应商并填 API Key(输入不回显), 写进数据目录的 `ricow.toml` 后直接进入会话;
- **零配置的第一步是回测**(不花钱、不需要交易所账号):

  ```bash
  ricow backtest --strategy <模板id> --pair <交易对> --days 30
  ```

  模板清单与交易对视野分别见 Web 策略面板 / `ricow pairs`。

## 4. 下一步

- 完整文档: 包内 `README.md` 或仓库 [README_zh.md](README_zh.md);
- 交易对视野(默认只列股票类): `ricow pairs`, 放开全部见 `[market] show_all_pairs`;
- 网络代理排障: README §6(国内网络通常需设 `HTTPS_PROXY`);
- 升级: 下载新压缩包覆盖, 或重跑安装脚本(程序**不含**自动更新组件)。
