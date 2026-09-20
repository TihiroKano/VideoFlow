# VideoFlow 下载功能方案

## 1. Bilibili 1080P+ 分辨率下载

- **输入方式**：Bilibili 视频网页链接。
- **解析引擎**：yt-dlp。
- **账号支持**：支持用户登录 Bilibili 账户，使用 Cookie 获取账号权限内的视频资源。
- **清晰度**：
  - 360P
  - 480P
  - 720P
  - 1080P
  - 1080P+
  - 4K（资源支持时）
  - 8K（资源支持时）
- **音视频处理**：视频流与音频流分离时，由 FFmpeg 自动合并。
- **下载引擎**：解析完成后统一交给 VideoFlow 原生 HTTP Engine。
- **下载能力**：多线程/分段下载、断点续传、失败重试、速度控制。
- **登录方式**：优先采用 Cookie/浏览器登录态导入，不保存用户账号密码。

---

## 2. 抖音分享链接下载

- **输入方式**：
  - 抖音分享链接
  - `v.douyin.com` 短链接
  - Douyin 视频网页链接

### 解析流程

```text
分享链接
↓
短链解析 / URL 重定向
↓
获取视频 ID
↓
Douyin Resolver
↓
获取媒体资源
↓
优先选择原生无水印资源
↓
VideoFlow HTTP Engine
↓
完成下载
```

- **下载模式**：用户无需手动寻找真实视频地址，只需粘贴分享链接。
- **视频信息**：标题、作者、封面、时长、分辨率。
- **无水印**：优先获取平台提供的无水印媒体资源；不存在时回退至可用资源。
- **下载引擎**：统一使用 VideoFlow 原生 HTTP Engine。
- **异常处理**：针对链接失效、Cookie、验证、解析失败等情况提供明确错误状态。
- **定位**：抖音属于**“分享链接 → 自动分析 → 视频资源 → 下载”**模式。

---

## 3. M3U8 下载 + MP4 转换

### M3U8 下载

```text
M3U8 URL
↓
M3U8 Parser
↓
识别 Master Playlist
↓
选择分辨率 / 视频流
↓
解析 Media Playlist
↓
获取 TS / fMP4 分片
↓
并发下载
↓
按分片序号整理
↓
FFmpeg 封装
```

### 支持内容

- Master M3U8
- 多分辨率选择
- TS 分片
- fMP4 / M4S 分片
- 音视频独立流
- AES-128 等常见非 DRM 加密形式
- 断点续传
- 分片失败重试
- 并发分片下载

### MP4 转换

```text
M3U8 / TS / fMP4
↓
FFmpeg
↓
封装 / 合并
↓
MP4
```

优先采用 **Remux（无损封装）**，无需重新编码时避免画质损失。

同时提供：

- M3U8 → MP4
- TS → MP4
- M4S/fMP4 → MP4
- 独立视频 + 音频 → MP4

---

## 4. 最终统一架构

```text
                    VideoFlow
                        │
                  URL / M3U8 输入
                        │
          ┌─────────────┼─────────────┐
          ↓             ↓             ↓
      Bilibili        Douyin        M3U8
       yt-dlp        Resolver       Parser
          │             │             │
          └─────────────┼─────────────┘
                        ↓
              Media Resource Layer
                        ↓
              Native HTTP Engine
                        ↓
        ┌───────────────┼───────────────┐
        ↓               ↓               ↓
     直链下载        分片下载        断点/并发
        │               │
        └───────────────┘
                        ↓
                     FFmpeg
                        ↓
              MP4 / MKV / 原始格式
```

### 核心原则

**解析与下载彻底分离：**

- **yt-dlp / Douyin Resolver / M3U8 Parser**：负责“找到资源”。
- **VideoFlow Native HTTP Engine**：负责高速、可靠地下载资源。
- **FFmpeg**：负责音视频合并、分片封装与格式转换。
