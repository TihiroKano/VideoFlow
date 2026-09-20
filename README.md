# VideoFlow

> 液态玻璃（Liquid Glass）风格的 Windows 桌面视频下载 / 格式转换工具。
> Tauri 2 + React 19 + WebGL2 自绘玻璃光学层，本地 FFmpeg 引擎，不依赖浏览器扩展。

![platform](https://img.shields.io/badge/platform-Windows%2010%2F11-0078D6)
![tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)
![react](https://img.shields.io/badge/React-19-61DAFB)
![rust](https://img.shields.io/badge/Rust-1.77%2B-000000)
![webgl](https://img.shields.io/badge/WebGL2-Liquid%20Glass-8A2BE2)
![license](https://img.shields.io/badge/license-Apache--2.0-blue)

<img src="src/assets/app-icon.png" width="120" alt="VideoFlow" />

---

## ✨ 特性

### 下载

- **多源解析**：站点直连解析 + `yt-dlp` 兜底 + **Browser Resolver**（独立 WebView2 抓取页面真实播放地址，覆盖需要 JS 计算签名的站点）
- **HLS / DASH**：清单交给 FFmpeg 拉取并无损封装（原生处理 AES-128、`EXT-X-BYTERANGE`、fMP4 init segment）
- **双引擎下载**：原生 HTTP（Range 分段并发、断点续传、暂停/恢复）+ HLS 流式
- **任务队列**：并发上限、按站点限流、优先级、失败重试、退出前把队列落盘，重启自动恢复
- **链接历史**：最近 100 条，去重并自动淘汰最旧

### 格式转换（本地转码）

- **视频容器**：MP4 / MKV / WebM / MOV / AVI / FLV
- **视频编码**：H.264 / H.265 / AV1 / VP9
- **音频编码**：AAC / Opus / MP3 / FLAC；音频提取：MP3 / M4A / FLAC / WAV（也能在音频格式之间互转）
- **容器与编码联动过滤**：不合法组合（`webm+h264`、`avi+h265` 这种「能写进去但解不出来」的坑）在界面上就选不出来
- **GPU 硬件加速**：H.264 / H.265 走 QSV / NVENC（实测 **4~5 倍**）；AV1 / VP9 自动回落软件；硬件路径失败会自动改用软件重跑
- **音质提示**：有损→无损、码率不降等情况在提交前给出说明

### 界面

- **Liquid Glass 光学层**：全局单一 WebGL2 画布，圆角矩形 SDF + 倒角高度场 + 双界面折射 + GGX 高光；三档质量（质量 / 平衡 / 性能），不可用时自动下沉到 CSS 毛玻璃
- 无边框窗口 + 自定义标题栏；滚动条自动隐藏；深浅背景自适应

---

## 🚀 快速开始

### 方式一：安装包（推荐）

从 [Releases](https://github.com/TihiroKano/VideoFlow/releases) 下载 `VideoFlow_x.y.z_x64-setup.exe`，
双击安装即可（当前用户安装，**不需要管理员权限**）。

> 安装包内已包含 `ffmpeg.exe` / `ffprobe.exe` / `yt-dlp.exe`，开箱即用。

### 方式二：从源码构建

**环境要求**

| 依赖 | 版本 | 说明 |
|---|---|---|
| Windows | 10 21H2+ / 11 | 需要 WebView2 运行时（系统自带） |
| Node.js | 18+ | 前端构建 |
| Rust | 1.77+ | 主进程；Windows 目标需 MSVC 或 MinGW 工具链 |
| FFmpeg / ffprobe / yt-dlp | — | 放进 `bin/`（见下） |

**准备 sidecar**

仓库不包含这三个可执行文件（体积原因），请自行下载后放到项目根目录的 `bin/`：

```
bin/
├── ffmpeg.exe      # 建议 full build：含 libx264/libx265/libvpx/libaom/libmp3lame/libopus，并带 QSV/NVENC 编码器
├── ffprobe.exe
└── yt-dlp.exe      # 需要 yt-dlp 兜底的站点解析时使用
```

程序按「exe 同级的 `bin/` → 源码树的 `bin/` → 当前目录的 `bin/`」顺序查找，安装包里则装在
`<安装目录>\bin\`。

**开发运行**

```bash
npm install
npm run app          # 等价于 tauri dev：起 Vite 开发服务并拉起桌面窗口
```

**打包**

```bash
npm run tauri build  # 产出 release 版与 NSIS 安装包
```

产物位置：`<cargo target>/release/videoflow.exe` 与
`<cargo target>/release/bundle/nsis/VideoFlow_<版本>_x64-setup.exe`。

**测试**

```bash
cargo test --no-default-features --lib   # Rust 单元测试：下载内核与 GUI 解耦，无需 Tauri 依赖
```

---

## 🧱 技术栈

| 层 | 选型 |
|---|---|
| 桌面壳 | Tauri 2（Rust 主进程 + WebView2） |
| 前端 | React 19 · TypeScript · Zustand · Vite |
| 玻璃光学层 | 自研 WebGL2 着色器（SDF + 折射 + GGX），全局单画布按需重绘 |
| 媒体处理 | FFmpeg / ffprobe / yt-dlp 作为 sidecar 子进程 |
| 样式 | 原生 CSS + 设计令牌（`src/design/tokens.css`） |

---

## 📁 目录结构

```
VideoFlow/
├── src/                     # 前端（React）
│   ├── components/          # 通用组件：glass（玻璃）、layout、feedback…
│   ├── features/            # 页面：compose（下载）· convert（转换）· library · settings
│   ├── services/            # IPC 客户端与领域类型
│   ├── stores/              # Zustand 状态
│   └── design/              # 设计令牌与样式
├── src-tauri/               # Rust 主进程
│   ├── src/resolver/        # 站点解析（含 Browser Resolver）
│   ├── src/downloader/      # 原生 HTTP / HLS 引擎
│   ├── src/media/           # FFmpeg 封装：remux / 合并 / 转码 / 能力表
│   ├── src/state.rs         # 任务调度与状态机
│   └── capabilities/        # Tauri 权限（窗口、对话框）
└── bin/                     # sidecar（不入库，见「准备 sidecar」）
```

---

## 📌 已知约束

- **仅 Windows**：界面与窗口控制按 Windows 设计（WebView2 + 无边框窗口）。
- **应用数据固定落在 `D:\VideoFlow`**：`settings.json` / `tasks.json` / `history.json` / `Downloads`
  都是硬编码路径（刻意的设计：不写 C 盘）。目标机器需要有 D 盘。
- **安装包体积较大**：源于内嵌的三个 sidecar（约 213 MB，压缩后安装包 ~75 MB）。
- **转码不支持断点续传**：暂停后继续会从头开始（FFmpeg 本身的限制），界面已如实说明。

---

## 📄 许可证

[Apache License 2.0](LICENSE) © 2026 TihiroKano