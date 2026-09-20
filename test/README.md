# test —— 全部测试与验证工具

> 这个目录整体删除不会影响应用构建与运行。
> 开发期夹具由 `src/features/dev/devFixtures.ts` 通过 `import.meta.glob` 动态发现，
> 目录不存在时静默跳过；生产构建也不会把夹具打进产物。

## 目录

```text
test/
├─ unit/                     前端纯函数单测（node --test，Node 24 原生支持 TS）
│  └─ urlValidation.test.ts
├─ fixtures/                 开发期视觉比对夹具
│  └─ previewState.ts        预览图所示的填充态（解析结果 + 三条任务 + 队列统计）
├─ _analysis/                视觉比对工具与产物（截图、并排图）
│  ├─ screenshot.ps1         抓取真实窗口（DPI 感知）
│  ├─ sidebyside.ps1         与 docs/预览图.png 生成并排比对图
│  ├─ compare.ps1            局部裁剪与取色
│  └─ *.png                  比对产物，可随时删除
└─ run-all.ps1               一键跑全部测试
```

Rust 侧测试位于 `src-tauri/`：单元测试在各模块的 `#[cfg(test)] mod tests`，
端到端测试在 `src-tauri/tests/download_e2e.rs`。

## 运行

```powershell
# 全部（不含网络用例）
powershell -ExecutionPolicy Bypass -File test\run-all.ps1

# 含真实网络下载用例
powershell -ExecutionPolicy Bypass -File test\run-all.ps1 -IncludeNetwork
```

单独运行：

```powershell
# 前端单测
node --test test/unit/urlValidation.test.ts

# Rust 单测（纯逻辑层，不链接 WebView2）
cd src-tauri
cargo test --no-default-features --lib

# 真实网络：分段下载、断点续传、端到端落盘
cargo test --no-default-features --lib -- --ignored
cargo test --no-default-features --test download_e2e -- --ignored --nocapture
```

> `--no-default-features` 会关闭 `gui` feature，只编译解析器 / 下载内核 / 存储层。
> 这样测试不需要链接 WebView2 等 GUI 依赖，也强制保持了「下载内核与界面解耦」。

## 视觉比对

用夹具把应用置于与 `docs/预览图.png` 相同的状态，再做同状态比对：

```powershell
# Tauri 窗口没有 URL query，用环境变量触发夹具
$env:VITE_VF_SEED='preview'
npm run app

# 另开一个终端抓图并生成并排图
powershell -ExecutionPolicy Bypass -File test\_analysis\screenshot.ps1
powershell -ExecutionPolicy Bypass -File test\_analysis\sidebyside.ps1
```

在浏览器中调试时可直接访问 `http://localhost:1420/?seed=preview`。

夹具仅在开发构建生效（`import.meta.env.DEV`），生产构建下加载分支会被打包器消除。