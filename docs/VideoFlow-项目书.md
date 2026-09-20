# VideoFlow 项目书

> 版本：1.1｜状态：实施基线｜目标平台：Windows 11 桌面端优先，响应式 Web 壳层可复用
>
> **视觉总原则：以用户参考图的视觉关系为验收对象。** 先匹配背景景深、半透明主容器、浮层卡片、圆角、留白、光边、控件密度与层级，再实现装饰性细节；不得用普通深色面板或高饱和渐变替代液态玻璃，从而造成“功能可用、视觉不像”的结果。

---

## 0. 文档边界、来源与决策

本项目书将原《方案书-LiquidGlass-AppUI-v4》**废止为独立文档**：其液态玻璃视觉、四层渲染、三级降级、Windows 11 壳层与真机验证纪律均迁入本书，成为唯一实施来源。参考图是视觉关系基准；其中“视频背景”与当前产品决定冲突，故不复制其动态功能，改为**用户自由上传的静态图片背景**，其余三栏布局、透明材质和控制层级按图校准。

| 分类 | 内容 |
|---|---|
| 来自原 v4 | Windows 11 的 Tauri 2 + WebView2 不透明无边框窗口；用户上传的应用内壁纸；单一 WebGL2 Canvas + Scissor 局部渲染；四层合成；SDF/双界面折射/三路采样/GGX 光照；L1/L2/L3 及可见降级诊断；真实 exe 截图验收。 |
| 来自用户参考图 | 左导航、顶部“视频下载/格式转换”模式切换、中央解析与规格选择、下方半透明下载队列、右侧背景控制、顶部窗口控制，以及蓝天城市背景上的浅蓝玻璃关系。 |
| 本次明确产品决定 | 背景允许用户自由上传 **JPG、PNG、WebP 静态图片**；不支持视频/动态壁纸。 |
| VideoFlow 新增 | 下载器产品定义、URL 解析、任务队列、分段下载、断点续传、文件管理、数据模型、状态机、接口、工程结构、测试与合规边界。 |
| 实施假设 | 下载能力仅用于用户有权下载的公开、无 DRM 资源；站点适配器须遵守目标站点条款、robots、授权和当地法律，**不实现 DRM 绕过、登录凭据窃取、付费墙/访问控制绕过**。 |

### 0.1 项目目标

VideoFlow 是一个“粘贴链接—确认资源—稳定下载—轻松管理”的桌面视频下载器。它把高频下载工作流置于视觉中心，同时将复杂的解析、队列、重试和文件恢复放到可靠的后台系统。成功标准：用户在 10 秒内完成首个合法任务创建；下载过程中 UI 始终可响应；任一失败任务有明确、可操作的恢复路径。

### 0.2 非目标

- 不提供 DRM 内容下载或破解能力，不规避地区、会员、验证、人机验证或访问控制。
- 不把“支持所有网站”作为承诺；以版本化、可禁用的受许可 Provider 适配器扩展。
- 首版不做剪辑、转码编辑器或云端同步；仅在格式合并/封装确有必要时调用本地媒体工具。

---

## 1. 产品定位与整体架构

### 1.1 用户与核心路径

| 用户 | 诉求 | 主路径 |
|---|---|---|
| 轻量下载者 | 快速保存一个视频 | 粘贴 URL → 解析 → 选质量/保存位置 → 下载完成 |
| 批量整理者 | 多任务、不中断、可追溯 | 批量导入 → 队列调度 → 暂停/恢复 → 打开/归档 |
| 内容创作者/研究者 | 可控格式与文件命名 | 解析元数据 → 选择流/字幕/命名模板 → 下载/合并 |

### 1.2 逻辑架构

```text
React UI / 设计系统
  ├─ 页面与视图状态（只展示，不直接下载）
  ├─ IPC/API Client
  └─ Liquid Glass Renderer（按档位渲染）
          │
桌面主进程 / 本地服务
  ├─ Command Handler（创建、暂停、恢复、取消）
  ├─ Provider Registry（可许可的 URL 解析适配器）
  ├─ Queue Scheduler（并发、优先级、网络策略）
  ├─ Download Engine（HTTP Range、校验、断点恢复）
  ├─ Media Pipeline（可选合并、封装、缩略图）
  ├─ File Manager（命名、冲突、清理、打开目录）
  └─ Persistence（SQLite + 临时分片）
          │
本地文件系统 / 受支持来源的公开接口
```

### 1.3 运行形态与技术栈

- **桌面壳**：Tauri 2 + Rust + 系统 WebView2；`transparent: false`、`decorations: false`、可缩放/可最大化，采用自绘标题栏。下载引擎不受前台页面生命周期影响。
- **前端**：React + TypeScript + Vite；Zustand 管理短生命周期 UI 状态，TanStack Query 管理主进程查询缓存。
- **渲染与动效**：Three.js（WebGL2）承载一个全局 Canvas、`paintComposeRT` 与 Scissor 局部玻璃渲染；CSS Custom Properties 与 Motion One 仅做语义动效。质量/平衡档以 WebGL2 能力为条件；性能档为 CSS 静态图片毛玻璃。
- **后端**：Rust（Tokio、Reqwest、SQLx/SQLite）；媒体探测与合并使用受控的本地 FFmpeg sidecar，版本锁定且记录来源。
- **质量保障**：Vitest、Playwright、Rust 单元/集成测试、Lighthouse/Chrome Performance、真实低配机基准。

---

## 2. 信息架构、页面与 UX

### 2.1 全局骨架

参考图应被转换为以下关系，而不是只复制单个组件：用户上传的静态图片沉浸式背景在最底层；半透明 App Shell 是最大的视觉锚点；左导航、中央解析工作台、下方任务队列、右侧背景控制与设置弹窗处于不同海拔的玻璃层；顶部模式切换与“解析/开始下载”是最清晰的操作焦点；文字始终保持可读、留白克制。

```text
不透明窗口（用户上传的静态图片 Background）
├─ 自绘标题栏：右上 设置 / 最小化 / 最大化 / 关闭
├─ 左侧玻璃导航：VideoFlow 标识；首页、下载记录、转换记录；分隔线；背景、设置
├─ 顶部中央模式分段控件：视频下载（默认） / 格式转换
├─ 中央工作台：URL 输入 + 解析；解析结果卡；清晰度/格式/音频；开始下载
├─ 下方玻璃队列：下载任务、全局入口、逐任务进度和动作
└─ 右侧玻璃控制台：当前图片缩略图；选择/更换图片；适配、透明度、模糊、亮度；循环播放/静音不实现
```

### 2.2 页面规格

#### 下载页（默认页）

- **空状态**：中央工作台使用大号 URL 输入胶囊、链接图标、粘贴按钮、解析按钮和支持来源提示；解析结果与清晰度/格式/音频区隐藏，下载队列保留。输入与解析按钮是唯一高亮主操作。
- **解析中**：输入锁定为可取消，出现紧凑骨架卡与“正在读取可下载信息”；不转整页加载，不遮挡已有任务。
- **解析成功**：显示封面、标题、时长、来源、可选格式/清晰度、估算大小、字幕选项、保存位置和“加入下载”。未知/不可验证的数据标为“待确认”，不虚构。
- **活跃任务**：按优先级和创建时间排列；顶部给出全局速度、剩余任务、全局暂停按钮。
- **批量输入**：粘贴多行 URL 时显示可展开的预检列表；可移除单项后再创建，错误项不阻塞合法项。

#### 转换、已完成与文件库

- 已完成默认按完成时间倒序；提供“打开文件”“定位目录”“复制来源链接”“重新下载”“移除记录”。
- 文件库仅管理 VideoFlow 已知文件，展示名称、类型、大小、日期、来源和状态；删除必须显示确切路径与二次确认，默认是移入系统回收站。
- 找不到文件时标记“文件已在外部移动”，保留记录并允许重新定位或移除记录。
- “格式转换”是独立模式：只接受用户已拥有的本地文件，以 FFmpeg 进行明确的格式/编码/音频选择；不得暗示转换能绕过受保护内容。其任务使用同一队列与文件管理模型，但类型为 `convert`。

#### 设置页

参考图的右侧面板在首页承担“背景”快捷控制：静态图片缩略图、选择/更换、`cover/contain` 适配、透明度、模糊程度、亮度与恢复默认。它不展示“背景类型：视频”、循环播放或静音。完整设置为独立玻璃抽屉/页面：下载（并发、网络、完成提醒）、文件（目录、命名、冲突策略）、外观（Liquid Glass 档位、减少动效）、辅助功能、隐私与日志。视觉档位切换有即时预览，但应用新档位时不得重启或中断下载。

### 2.3 可复用组件

| 组件 | 状态/规则 |
|---|---|
| `UrlComposer` | idle / focus / invalid / parsing / parsed；粘贴即规范化 URL，提交前再校验；Enter 解析，Esc 清空或关闭结果。 |
| `MediaPreviewCard` | loading / success / unsupported / auth-required / error；封面懒加载，失败用来源色块与图标替代。 |
| `DownloadTaskCard` | queued / downloading / paused / retrying / merging / completed / failed / cancelled；进度、速度、剩余时间、原因和下一步始终可见。 |
| `QualityPicker` | 有可用选项才可选；展示容器、分辨率、帧率、音频和估算大小。 |
| `GlassSurface` | `quality | balanced | performance` 变体；内容层不依赖滤镜保证可读。 |
| `Toast / InlineNotice` | 成功可自动淡出；失败、有数据损失或权限问题必须持久显示且可复盘。 |
| `ConfirmDialog` | 取消下载、删除文件、清空记录等危险动作；焦点锁定、Esc 关闭（删除文件例外：须点按钮）。 |

### 2.4 关键交互准则

- 一项命令只改变一个明确状态；按钮文案用动词：解析、加入下载、暂停、恢复、取消、打开文件。
- 暂停/恢复为即时乐观反馈，后台确认失败时回滚并显示原因。
- 取消下载仅删除临时分片，不删除已经完成文件；“删除文件”是另一个受确认动作。
- 自动重试不抢占用户暂停；用户取消后不自动重试。
- 所有失败都提供“重试”“查看详情”以及可用时的“重新解析/换保存位置”。

---

## 3. 下载功能实现方案

### 3.1 URL 输入、校验与解析

1. 前端去空格、提取多行 URL、用 `URL` 构造器检查 scheme（仅 `https:`，如有明确需求再允许 `http:`），限制单次 50 条、单 URL 2,048 字符。
2. 主进程做 SSRF 防护：拒绝 localhost、私网、回环、链路本地与文件协议；DNS 解析后再次校验目标 IP；禁止自动跳转到不安全地址。
3. `ProviderRegistry` 根据域名/路径选择**允许且已启用**的 Provider。Provider 只返回标准化 `ResolvedMedia`，不把站点 DOM 逻辑泄漏给 UI。
4. 解析结果有 TTL（默认 15 分钟）与来源版本；过期、签名 URL 失效或开始下载 403 时转 `needs_reparse`，用户确认后重新解析。
5. 不支持、受 DRM 保护、需登录/授权或政策禁止时，返回明确的分类错误和合规说明，绝不尝试绕过。

### 3.2 队列与调度

- 默认并发下载 3 个，单任务分段并发默认 4，均可在设置调为 1～6/1～8；设备低档或网络受限时自动下调。
- 优先级：用户手动置顶 > 正在恢复 > 普通队列；同一来源可设置每域名并发上限，避免触发服务端限流。
- Scheduler 维护持久化队列：应用关闭时运行中任务安全落盘为 `paused`；下次启动校验临时文件和 ETag/Last-Modified 后恢复。
- 网络离线、系统睡眠、磁盘不足时停止新的分段请求，将任务置为可恢复暂停并通知用户；网络恢复不应自动恢复“用户手动暂停”的任务。

### 3.3 下载、断点续传与合并

- 优先使用 HTTPS 和响应头 `Accept-Ranges: bytes`。支持 Range 的资源按固定或自适应 chunk（8–32 MiB）写入 `<task>.part` 与 `<task>.resume.json`；每完成一段原子更新检查点。
- 无 Range 的资源以单流下载；断线后若服务器无法续传，明确提示“需从头重下”，并由用户选择继续。
- 对每个请求设置连接、首字节、读超时与指数退避（1s、2s、4s，最多 3 次；429/503 尊重 `Retry-After`）。不对 4xx、内容不匹配、磁盘错误盲目重试。
- 完成后进行长度校验；Provider 提供时校验 SHA-256；流式媒体按声明顺序下载并由 FFmpeg 仅做无损 remux/合并。失败不覆盖原文件。
- 成功文件先写同目录临时名，校验后原子 rename；文件名冲突采用 `name (1).ext` 或用户指定的覆盖策略（默认绝不覆盖）。

### 3.4 进度与控制

进度事件最大 4 次/秒合并推送，UI 使用插值而非每字节重渲染。未知总大小显示已下载字节与不确定进度条；速度为 5 秒 EWMA，ETA 仅在总量、速度稳定且大于 0 时展示。

| 操作 | 下载引擎行为 | UI 反馈 |
|---|---|---|
| 暂停 | 停止创建请求，等待/取消在途请求，落盘检查点 | 状态变“已暂停”，显示可恢复数据大小 |
| 恢复 | 校验 token、文件、远端版本后重新排队 | “正在验证”→队列/下载中 |
| 取消 | 取消请求，删除临时分片和 resume 元数据 | 移入“已取消”；不会触碰最终文件 |
| 重试 | 保留失败上下文，必要时重新解析 | 重试计数与新错误可审计 |
| 打开 | 只允许已完成且路径存在 | 调用系统默认打开/显示于目录 |

### 3.5 错误分类与用户文案

| 代码域 | 示例 | 默认动作 | 用户可做什么 |
|---|---|---|---|
| `URL_*` | 格式错误、不安全地址 | 不入队 | 修改链接 |
| `PROVIDER_*` | 不支持、解析过期、受限 | 不重试 | 重新解析、查看支持说明 |
| `NETWORK_*` | 断网、超时、429 | 条件重试后暂停 | 重试、检查网络 |
| `HTTP_*` | 403、404、5xx | 403/404 不盲重试 | 重新解析/换链接 |
| `STORAGE_*` | 权限、空间不足、路径不可写 | 立即暂停 | 更换目录、释放空间 |
| `MEDIA_*` | 合并失败、格式不兼容 | 保留源分片与日志 | 重试、导出诊断信息 |
| `INTEGRITY_*` | 长度/哈希不符 | 删除不可信最终产物 | 重新下载 |

错误详情须展示可读摘要、技术代码、时间、任务 ID 和脱敏日志；URL query、cookie、授权头永不写入 UI 或普通日志。

---

## 4. 数据模型、状态机与接口

### 4.1 核心数据模型（SQLite）

```ts
type TaskStatus =
  | 'draft' | 'resolving' | 'queued' | 'downloading' | 'pausing'
  | 'paused' | 'retry_wait' | 'verifying' | 'merging'
  | 'completed' | 'failed' | 'cancelled' | 'needs_reparse';

interface DownloadTask {
  id: string; sourceUrl: string; canonicalUrl?: string; providerId?: string;
  status: TaskStatus; priority: number; createdAt: string; updatedAt: string;
  media?: ResolvedMedia; selection?: StreamSelection; targetPath?: string;
  downloadedBytes: number; totalBytes?: number; retryCount: number;
  error?: TaskError; resume: ResumeCheckpoint; version: number;
}

interface ResolvedMedia {
  title: string; sourceName: string; thumbnailUrl?: string; durationSec?: number;
  streams: MediaStream[]; subtitles: SubtitleTrack[]; expiresAt?: string;
}

interface TaskError { code: string; message: string; retryable: boolean; detail?: string; }
interface ResumeCheckpoint { etag?: string; lastModified?: string; chunks: ChunkState[]; tempPath?: string; }
```

表：`tasks`、`task_streams`、`task_chunks`、`settings`、`file_records`、`event_log`。敏感会话信息不进入 SQLite；若未来授权 Provider 需要凭据，使用系统 Keychain/Credential Manager，并要求用户显式登录。

### 4.2 状态机

```text
draft → resolving → queued → downloading → verifying → [merging] → completed
                    │          │  │               └→ failed
                    │          │  └→ pausing → paused → queued
                    │          └→ retry_wait → queued / failed
                    └→ needs_reparse → resolving / cancelled
任意非终态 ─取消→ cancelled
终态 completed/failed/cancelled 仅能通过“新建/重试/重新下载”产生新运行记录
```

后端是状态真相源。每次转换采用事务式 compare-and-swap（`version`），拒绝过期命令；前端只可显示乐观中间态，收到 `task.updated` 后收敛。

### 4.3 IPC / 事件契约

```ts
// Commands: 请求-响应，所有 mutation 需 idempotencyKey
resolveUrls({ urls }): Promise<ResolveResult[]>;
createTask({ resolvedId, selection, targetDir, idempotencyKey }): Promise<DownloadTask>;
taskAction({ taskId, action: 'pause'|'resume'|'cancel'|'retry', idempotencyKey }): Promise<DownloadTask>;
setSetting({ key, value }): Promise<void>;
openFile({ fileRecordId }): Promise<void>;

// Events: 可重放序号，前端以 seq 去重
// 事件名只允许 [A-Za-z0-9] 与 - / : _（Tauri 的限制，带 `.` 会被判非法名而静默不推送），
// 因此统一用下划线分隔。
'task_updated'({ seq, task });
'task_progress'({ seq, taskId, downloadedBytes, totalBytes?, speedBps, etaSec? });
'task_removed'({ seq, taskId });
'queue_updated'({ seq, activeCount, queuedCount });
'app_notice'({ seq, level, code, message, taskId? });
```

任何 command 失败均返回 `{ code, message, retryable, correlationId }`；不要以字符串匹配驱动 UI。事件断线后调用 `getSnapshot(afterSeq)` 补齐。

---

## 5. Liquid Glass 视觉系统（并入 VideoFlow）

### 5.1 四层合成与局部渲染（v4 迁入）

合成顺序固定为 **静态图片模糊基底 → 液态玻璃容器 → DOM 内容 → 高亮反馈**。这是可读性与真实光路的边界，不得调换。

1. **模糊基底层**：用户上传的 JPG/PNG/WebP 静态图片先被尺寸限制/降采样，上传为 WebGL 纹理，渲染到 `paintComposeRT` 并生成 mip 层级。背景永不承载交互文字；它是所有折射的唯一背景来源。
2. **液态玻璃容器层**：一个全局 Three.js WebGL2 Canvas，以 Scissor/局部 RT 仅绘制左导航、顶部模式控件、输入容器、解析卡、规格选择、下载队列、右侧背景面板和悬浮菜单。禁止每个组件创建一个 WebGL context。
3. **DOM 内容层**：标题、图标、输入框、表单、进度、列表和所有命中区最后绘制；它们不参与折射和模糊，确保清晰可读。
4. **高亮反馈层**：静态高光（边缘 GGX/Fresnel）在玻璃 shader 内计算；hover、按压波纹和鼠标跟随高光用 CSS 覆盖，必须 `pointer-events: none`，不参与折射。

液态感来自“真实背景采样的形变 + 受光倒角 + 半透明层叠”，而不是在 DOM 上叠多层 `backdrop-filter`。组件 SDF 坐标按自身尺寸归一化（`p = (uv - .5) * vec2(componentAspect, 1)`），窗口自由缩放、跨 DPI 移动或右侧面板展开时，圆角、倒角与高度场不得拉扁。

### 5.1.1 质量/平衡档的光学管线（v4 迁入）

- **轮廓**：圆/圆角矩形 SDF；`fwidth` 抗锯齿；视觉噪声 SDF 与法线用的平滑 `glassSurfaceSdf` 分离。
- **高度/法线**：`height = thickness * quintic(clamp(interior / bevel, 0, 1))`；中心必须平坦（法线恒为 `(0,0,1)`），只有倒角连续弯曲。法线采样步长须 clamp，不能直接对含噪 Mask 求导。
- **折射**：按 Air → Glass → Air 追踪前表面入射、玻璃内实际光程、平背面出射与背景平面相交。默认 IOR 1.46；不使用 `normal.xy * strength` 这种假偏移。R/G/B 色散仅用于边缘。
- **三路采样**：Face 保持中心稳定与较高 LOD；Inner Rim 负责边缘压缩（低 LOD）；Outer Rim 向外读取背景并更柔和（高 LOD）；权重由 SDF 倒角带归一化。中心禁止明显色散。
- **光照顺序**：`REFRACT → SHADOW → TINT → LIGHT`；最后才叠 GGX + Smith + Schlick 边缘高光。高光/Fresnel 限在倒角外半段，中心不能鼓成塑料白斑。

### 5.2 三档规范

| 项目 | 质量 `quality` | 平衡 `balanced` | 性能 `performance` |
|---|---|---|---|
| 目标设备 | 独显/高性能集显，WebGL2、RT 与 `textureLod` 能力完整 | 主流集显/笔记本，WebGL2 可用但性能/能力有限 | 低配、远程桌面、节能或 WebGL2 不可用 |
| 实现级别 | **L1** 完整管线：四层、SDF、高度场、双界面、三路采样、GGX | **L2** 简化双界面：低分辨率 RT/LOD，保留玻璃形体与折射 | **L3** 纯 CSS：应用内静态图片的预模糊副本 + 半透明底色 |
| 模糊 | `paintComposeRT` 的完整 mip 采样，约 24–32px 感知半径 | 降分辨率 RT、较少 LOD，约 16–22px | CSS 背景副本 12–16px；不依赖透明窗口的 `backdrop-filter` |
| 折射 | 实际光程、IOR 1.46、边缘 RGB 色散 | 保留双界面折射；关闭色散与部分 BRDF | 无折射、无 Canvas/WebGL |
| 光照 | GGX/Smith/Schlick、四点缓慢掠射柔光、局部 CSS 高亮 | 固定主光、简化 Fresnel，低频高光 | 1px 内描边、静态阴影，无连续光照 |
| 动画 | 60fps 目标；仅指针附近局部响应；后台静止 | 30fps 上限；仅关键容器 | CSS opacity/transform 150–200ms；无连续视觉动画 |
| 资源上限 | GPU 额外预算约 10ms/frame；CPU UI < 8ms/frame | GPU < 6ms/frame；CPU UI < 6ms/frame | 不新增 WebGL；CPU UI < 4ms/frame |
| 任务下载优先级 | 下载繁忙时降采样/停止环境动画 | 下载繁忙时冻结折射 | 不受下载任务影响 |

所有档位共享相同布局、色彩语义、文字对比和交互语义，档位只能改变渲染成本，不能改变功能或让内容消失。

### 5.3 Token（CSS 自定义属性）

```css
:root {
  --vf-bg: #0b5e9f; --vf-accent: #82c9ff; --vf-text: #f5f9ff;
  --vf-text-muted: color-mix(in srgb, var(--vf-text) 68%, transparent);
  --vf-success: #66d9a2; --vf-warning: #ffd17c; --vf-danger: #ff8a9d;
  --vf-radius-shell: 28px; --vf-radius-card: 18px; --vf-radius-control: 14px;
  --vf-space-1: 4px; --vf-space-2: 8px; --vf-space-3: 12px; --vf-space-4: 16px; --vf-space-6: 24px;
  --vf-glass-fill: rgb(238 245 255 / 12%);
  --vf-glass-border: rgb(255 255 255 / 25%);
  --vf-shadow-float: 0 16px 48px rgb(0 0 0 / 24%);
  --vf-focus: 0 0 0 3px rgb(143 184 255 / 66%);
}
```

参考图校准基准：天空蓝至深蓝的高动态静态图片背景；玻璃为偏冷的乳白/浅蓝半透明，亮边来自左上与右上，中心工作台比侧栏更亮，下载队列比解析卡更低海拔。上述初值用于开发，不得在组件内散落硬编码色值、模糊半径和圆角。

### 5.4 动画规范

- 页面/卡片进入：`opacity + transform`，180–240ms，`cubic-bezier(.2,.8,.2,1)`；不采用弹跳。
- 按钮 hover：150ms；按下缩放不低于 `0.98`；任务进度仅做数值/宽度平滑。
- Dialog：180ms；焦点立即转移，动画不延迟可操作性。
- `prefers-reduced-motion: reduce` 或应用“减少动效”时：取消环境扫光、折射位移、列表进入动画；保留 0–100ms 的必要状态淡变。
- 不使用滚动绑定的高频 layout 重算；动画仅限 `transform`、`opacity` 和合成层可处理的属性。

---

## 6. 首次启动设备检测与降级

### 6.1 检测流程

首次启动先用 `auto` 档运行一个 < 1 秒、无感的能力探测；不上传硬件指纹，只将推荐结果与简短理由保存在本地。弹出非阻塞提示：“已为此设备推荐平衡效果，可随时在设置中更改”。用户一旦手动选择，保存 `manual` 覆盖自动结果。

| 信号 | 获取方式 | 用途 |
|---|---|---|
| GPU 名称、专用/共享显存、驱动/图形 API | Tauri/Rust 平台信息（Windows DXGI、macOS Metal 等） | 设备分级；不可得则降权 |
| CPU 核心、架构、负载 | 原生系统信息 | 判断后台下载与渲染争用 |
| 可用内存/总内存 | 原生系统信息 | 防止多层纹理挤压内存 |
| WebGL2、`EXT_color_buffer_float`、`textureLod`、renderer、最大纹理 | 前端 feature detection + 短时离屏测试 | 决定 L1/L2 是否可启用 |
| 帧时间 | 120 帧 rAF 微基准，取 p95 | 检验真实组合性能 |
| 偏好与环境 | 节能模式、远程桌面、减少动效、浏览器 GPU 加速 | 直接保守降级 |

### 6.2 推荐算法（可解释且保守）

1. 若远程会话、GPU 加速不可用、WebGL2 不可用、`prefers-reduced-motion`、内存 < 4GB，推荐 **性能（L3）**。
2. 若专用显存 ≥ 4GB（或共享显存充足）、内存 ≥ 16GB、核心 ≥ 8、WebGL2 + RT/LOD 能力通过且 p95 帧时间 ≤ 18ms，推荐 **质量（L1）**。
3. 其余配置推荐 **平衡**；若 p95 帧时间 > 28ms 或可用内存紧张，降为性能。
4. 下载进行中每 10 秒只读一次渲染健康度：连续 3 个窗口掉帧/长任务（>50ms）过多时，自动档可按“L1→L2→L3”下沉；**不擅自改变用户已手选档位**，仅提示可切换。L1↔L2 必须带回滞、幂等且可逆，禁止抖动。

检测结果示例：`recommendation: balanced`、`reasons: ["WebGL2 可用", "内存 8 GB", "帧时间稳定但不建议实时多层折射"]`。硬件数据不可得不是错误，采用性能优先的保守默认值。

### 6.3 降级矩阵

```text
质量 L1 →（帧预算超限/节能）→ 平衡 L2 →（GPU 不可用/远程桌面）→ 性能 L3
性能 →（用户手动切换且能力检测通过）→ 平衡/质量
性能 L3：CSS 静态图片毛玻璃不可用 → 半透明实色 + 描边 + 阴影
任意档：高对比度模式 → 实色背景 + 语义色 + 取消透明材质
```

运行时状态不可静默：设置页及诊断 HUD 必须显示 `L1/L2/L3`、当前原因、探测结果与帧时间；L2/L3 下沉事件写入本地 `log_diag`。生产默认将 HUD 折叠为设置页“渲染状态”，测试/Beta 构建常显，确保真实 exe 中可以判断是否发生降级。

---

## 7. 可访问性、响应式与隐私

### 7.1 可访问性

- 正文与背景最低 4.5:1，大号文字/图标最低 3:1；玻璃效果导致不足时，增加内容背板，不降低文字透明度。
- 完整键盘路径：侧栏、输入、质量选择、任务动作、弹窗；焦点可见且不被透明高光吞没。
- `aria-live="polite"` 仅播报重要状态（解析完成、下载完成、需操作失败），进度每 10% 或 30 秒最多一次，避免读屏轰炸。
- 用图标加文字或 `aria-label`，不以颜色作为唯一状态提示；支持 200% 缩放与系统高对比模式。

### 7.2 响应式

- ≥ 1024px：侧栏 220–256px，主内容最大宽度 1,280px，任务卡双栏元数据。
- 720–1023px：窄侧栏仅图标或顶部导航，解析结果改为单栏。
- < 720px：若提供 Web 壳层，底部导航、全宽输入、任务卡堆叠；桌面应用最小窗口宽度 720px，低于时显示可读的紧凑布局。
- 不以固定像素截断标题；允许 2 行并提供完整 tooltip/可复制标题。

### 7.3 隐私与安全

- 默认本地优先：任务、文件索引、设备推荐、日志仅本机保存。
- 不收集完整 URL、文件名或硬件标识用于遥测；若用户选择诊断上传，先展示可预览、可编辑的脱敏包。
- Provider 请求最小化 header；不跟随跨域携带授权信息的重定向；记录第三方网络访问清单。
- 输出目录使用规范化路径与允许目录校验，防范路径穿越；sidecar 参数固定化，禁止把 URL/文件名拼接为 shell 命令。

---

## 8. 工程模块与目录

```text
videoflow/
├─ apps/desktop/                    # Tauri 壳、窗口与 IPC 注册
├─ apps/web/                        # React UI
│  └─ src/
│     ├─ features/{compose,tasks,library,settings}/
│     ├─ components/{glass,task,feedback}/
│     ├─ design/{tokens.css,glass.css,motion.ts}/
│     ├─ stores/  └─ services/ipc.ts
├─ crates/
│  ├─ core/                         # 领域模型、状态机、错误码
│  ├─ resolver/                     # Provider trait 与受支持适配器
│  ├─ downloader/                   # Range、分片、校验、重试
│  ├─ scheduler/                    # 队列、并发、策略
│  ├─ media/                        # 探测、合并、封装
│  ├─ storage/                      # SQLite、文件、恢复点
│  └─ platform/                     # 硬件能力、打开文件、回收站
├─ packages/contracts/               # IPC schema、类型与错误码
├─ tests/{unit,integration,e2e,fixtures}/
└─ docs/{providers,adr,performance}/
```

Provider 接口只接受/返回领域对象：`canHandle(url)`、`resolve(url, context)`、`validate(stream)`。适配器必须有许可记录、测试 fixture、版本和远程禁用开关；不得在 UI 里散写 Provider 分支。

---

## 9. 开发阶段与验收

### 9.1 阶段计划

| 阶段 | 交付 | 出口标准 |
|---|---|---|
| P0：视觉校准 | 导入参考图、布局标注、token 初稿、可点击静态原型 | 与图的层级/间距/材质经人工叠图确认；获得明确偏差清单 |
| P1：骨架与持久化 | Shell、导航、任务表、IPC contract、SQLite migration | 重启后任务/设置可恢复；无下载逻辑泄漏到 UI |
| P2：单任务闭环 | URL 校验、合法 Provider 解析、下载、进度、完成文件 | 成功/失败/取消可完整演示，文件不损坏 |
| P3：可靠队列 | 分段、断点续传、并发、重试、磁盘/网络处理 | 网络中断与重启恢复测试通过 |
| P4：Liquid Glass | 三档渲染、自动检测、降级、无障碍模式 | 设备推荐可解释、切换不影响下载 |
| P5：文件库与打磨 | 搜索、定位、回收站删除、日志、批量导入 | 端到端回归、性能与可访问性达标 |
| P6：Beta | 签名、更新、崩溃报告（可选）、反馈闭环 | 无 P0/P1 缺陷，发布清单完成 |

### 9.2 测试与验收标准

**功能**

- URL：空、无效、批量、重复、危险内网、受支持、过期签名、受限资源均有确定行为。
- 下载：支持 Range/不支持 Range、已知/未知大小、暂停恢复、应用重启、网络切换、429、磁盘满、同名文件、校验失败、合并失败都可复现并有正确恢复路径。
- 队列：并发上限、手动优先级、用户暂停不自动恢复、取消不删除最终文件、重复命令幂等。
- 文件：完成后可打开/定位；外部移动检测正确；删除仅影响指定文件并进入回收站。

**视觉与体验**

- 使用参考图在 1440×900、1280×800、1024×768 做叠图/并排复核：主容器比例、导航占比、输入区视觉重量、卡片层次、圆角、模糊边界、阴影方向和主要操作焦点为必测项。
- 三档功能布局一致；性能档绝不初始化 WebGL；质量/平衡在不支持时无白屏、黑屏或文字不可读。
- 键盘、读屏、减少动效、200% 缩放、高对比度均有测试记录。

**性能指标（发布基线）**

| 指标 | 质量 | 平衡 | 性能 |
|---|---:|---:|---:|
| 空闲交互帧率目标 | 60fps | 60fps | 60fps |
| 列表滚动 p95 帧时间 | ≤ 20ms | ≤ 18ms | ≤ 16ms |
| 20 个任务 UI 内存增量 | ≤ 180MB | ≤ 130MB | ≤ 90MB |
| 下载中点击/暂停响应 | ≤ 150ms | ≤ 150ms | ≤ 120ms |
| 进度事件 UI 更新 | ≤ 4 次/秒/任务，不卡主线程 | 同左 | 同左 |

基准应在高、中、低三类真实设备和 GPU 禁用环境执行；未达到帧预算时，优先削减液态玻璃频率/采样，而不是牺牲下载可靠性或文字可读性。

---

## 10. 实施决策清单

1. 以本项目书替代独立 v4 文档；任何新玻璃参数都写入 token 与三档表，而非另建分叉方案。
2. 先完成 P0 参考图校准，再冻结布局；不要在没有视觉基线时只靠文字描述“复刻”。
3. 下载内核与渲染层完全解耦：视觉降级、窗口最小化、页面切换都不得暂停后台任务。
4. 所有下载来源走显式许可/合规 Provider；不支持时要诚实、清楚地解释原因。
5. 自动推荐是建议而非限制，用户手动选择永久优先；“性能”始终是可靠的 CSS 基线。

## 11. 已纳入的输入与 P0 视觉验收清单

已纳入：v4 原方案、1280×800 参考图、以及“用户自由上传静态背景图片、非动态”的产品决定。P0 不再等待附件，而是输出以下可复核物：

- 以 1280×800 为主基准的叠图截图：左栏约 244px、中央工作台约 896px、右侧面板约 292px；数值允许按最小窗口自适应，但三栏主从关系不得改变。
- 对照截图中顶部模式分段控件、URL 胶囊和解析按钮、视频预览/元数据卡、三项规格选择、主下载按钮、下方三行任务队列和右侧图片控制台逐项标注“匹配/差异/原因”。
- 静态图片背景在 `cover`、`contain`、极窄和极宽图片下的截图；图片切换不得短暂露出桌面或透明黑底。
- L1、L2、L3 真实 Tauri exe 截图与 `log_diag`；L3 必须仍显示图片毛玻璃而非透明窗口后的桌面。

仍需产品负责人确认的仅有：品牌正式 Logo/字标、默认静态壁纸的授权来源、首批已授权 Provider 范围及格式转换的首发格式列表。这些只影响资源与 Provider 配置，不改变本文的架构、可靠性、合规和交互契约。
