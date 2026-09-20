//! 转码能力表：容器、编码、以及它们之间**真实可用**的组合。
//!
//! 这里是转码选项的单一真相源，同时供两处使用：
//! - `commands::convert::convert_capabilities` 下发给界面做联动过滤；
//! - `validate_plan` 在任务创建时做后端二次校验。
//!
//! ## 为什么必须有这张表（不是防御性编程）
//!
//! 容器与编码**不是自由组合**，实测（`bin/ffmpeg.exe` 逐个写入后完整解码验证）：
//!
//! | 容器 | 视频 | 音频 |
//! |---|---|---|
//! | mp4 / mkv | h264 h265 av1 vp9 | aac opus mp3 flac |
//! | webm | **仅 vp9 / av1** | **仅 opus** |
//! | mov | **仅 h264 / h265** | **仅 aac / mp3** |
//! | avi | h264 av1 vp9（**h265 排除**） | aac mp3 flac |
//! | flv | h264 h265 av1 vp9 | aac opus mp3 flac |
//!
//! 两类会害到用户的组合：
//! - **直接被 muxer 拒绝**：webm+h264、webm+aac、mov+av1、mov+opus → ffmpeg 报错，用户白等一场；
//! - **能写进去但文件是坏的**：`avi` + H.265 **写入成功、完整解码却报错**，而 ffmpeg
//!   **不返回任何错误**。这类只能靠黑名单拦——正是用户此前遇到的「下载后打不开」
//!   同一类问题，绝不能在这里重演。

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use serde::Serialize;

use crate::core::model::{ConvertMode, ConvertPlan};
use crate::error::{AppError, AppResult};
use crate::platform::sidecar;

/// 一个可选的目标容器
#[derive(Debug, Clone, Copy)]
pub struct ContainerSpec {
    /// 稳定标识，与 `ConvertPlan.container` 一致
    pub key: &'static str,
    /// 界面显示名
    pub label: &'static str,
    /// FFmpeg 的 `-f` 取值
    pub muxer: &'static str,
    /// 产物扩展名
    pub ext: &'static str,
    /// 允许的视频编码 key 列表；空表示不支持视频（纯音频目标）
    pub video: &'static [&'static str],
    /// 允许的音频编码 key 列表
    pub audio: &'static [&'static str],
}

/// 硬件（GPU）编码路径
#[derive(Debug, Clone, Copy)]
pub struct HardwareCodec {
    /// FFmpeg 编码器名，例如 `h264_qsv`
    pub encoder: &'static str,
    /// 界面上的短名（`QSV` / `NVENC`），用于下拉标注与任务卡
    pub label: &'static str,
    /// 恒定质量参数。**每条路径各自独立，绝不能互相套用**：
    /// QSV 只认 `-global_quality`，NVENC 用 `-rc vbr -cq`；
    /// 喂错不会报错，只会被静默忽略，结果是「能跑但质量完全不受控」。
    pub quality: &'static [&'static str],
    /// 实验性路径：参数按官方通用写法给，但本机没有对应硬件、没能实测验证过。
    /// 界面上要标出来，让用户知道这条路的把握不如已验证的那条。
    pub experimental: bool,
}

/// 视频编码
#[derive(Debug, Clone, Copy)]
pub struct VideoCodecSpec {
    pub key: &'static str,
    pub label: &'static str,
    /// FFmpeg 编码器名（软件）
    pub encoder: &'static str,
    /// 该编码器的恒定质量参数，形如 `-crf 23`
    pub quality: &'static [&'static str],
    /// 硬件候选，**按优先级排列**：逐个探测，取第一个真能编出帧的。
    /// 空表示这个编码只能软件编。
    pub hardware: &'static [HardwareCodec],
}

/// 音频编码
#[derive(Debug, Clone, Copy)]
pub struct AudioCodecSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub encoder: &'static str,
    /// 码率（kbps）的 `-b:a` 取值；无损编码为空
    pub bitrate: &'static str,
    /// 是否无损编码（决定「转过去会不会掉质量」的判断）
    pub lossless: bool,
}

/// 硬件编码只挂 H.264 / H.265 两条路，理由是**实测**而非取舍：
///
/// | 编码 | 硬件编码器 | 本机结果 |
/// |---|---|---|
/// | H.264 | `h264_qsv` | 可用（1080p/20s：7.3s vs 软件 34.0s，约 4.7 倍） |
/// | H.265 | `hevc_qsv` | 可用（同素材 14.0s vs 软件 47.1s，约 3.4 倍） |
/// | AV1 | `av1_qsv` | **运行时直接拒绝**：`This version of runtime doesn't support AV1 encoding` |
/// | VP9 | `vp9_qsv` | **运行时直接拒绝**：`Selected ratecontrol mode is unsupported` |
///
/// 质量：`h264_qsv -global_quality 23` 的 PSNR 45.33 略优于 `libx264 -crf 23` 的 44.71，
/// 也就是说硬件路径不是「用画质换速度」。
///
/// **NVENC 是实验性候选**：参数按 NVIDIA 官方通用写法（`-rc vbr -cq`，
/// 与 QSV 的 `-global_quality` 完全独立），但本机没有可用的 NVENC
/// （缺 `nvEncodeAPI64.dll`），没能实测验证品质与体积。它会排在 QSV 之后，
/// 只在 QSV 不可用、而本机 NVENC 探测通过时才被选中。
pub const H264_HARDWARE: &[HardwareCodec] = &[
    HardwareCodec {
        encoder: "h264_qsv",
        label: "QSV",
        quality: &["-global_quality", "23"],
        experimental: false,
    },
    HardwareCodec {
        encoder: "h264_nvenc",
        label: "NVENC",
        // NVENC 的恒定质量模式：vbr + cq，b:v 0 表示不设码率上限
        quality: &["-rc", "vbr", "-cq", "23", "-b:v", "0"],
        experimental: true,
    },
];

pub const H265_HARDWARE: &[HardwareCodec] = &[
    HardwareCodec {
        encoder: "hevc_qsv",
        label: "QSV",
        quality: &["-global_quality", "28"],
        experimental: false,
    },
    HardwareCodec {
        encoder: "hevc_nvenc",
        label: "NVENC",
        quality: &["-rc", "vbr", "-cq", "28", "-b:v", "0"],
        experimental: true,
    },
];

pub const VIDEO_CODECS: &[VideoCodecSpec] = &[
    VideoCodecSpec {
        key: "h264",
        label: "H.264",
        encoder: "libx264",
        quality: &["-crf", "23"],
        hardware: H264_HARDWARE,
    },
    VideoCodecSpec {
        key: "h265",
        label: "H.265 / HEVC",
        encoder: "libx265",
        quality: &["-crf", "28"],
        hardware: H265_HARDWARE,
    },
    VideoCodecSpec {
        key: "av1",
        label: "AV1",
        encoder: "libaom-av1",
        // AV1 的 CRF 标度与 x264 不同，且必须给足 cpu-used 否则慢到不可用
        quality: &["-crf", "32", "-cpu-used", "6"],
        hardware: &[],
    },
    VideoCodecSpec {
        key: "vp9",
        label: "VP9",
        encoder: "libvpx-vp9",
        quality: &["-crf", "32", "-b:v", "0"],
        hardware: &[],
    },
];

pub const AUDIO_CODECS: &[AudioCodecSpec] = &[
    AudioCodecSpec {
        key: "aac",
        label: "AAC",
        encoder: "aac",
        bitrate: "192k",
        lossless: false,
    },
    AudioCodecSpec {
        key: "opus",
        label: "Opus",
        encoder: "libopus",
        bitrate: "160k",
        lossless: false,
    },
    AudioCodecSpec {
        key: "mp3",
        label: "MP3",
        encoder: "libmp3lame",
        bitrate: "192k",
        lossless: false,
    },
    AudioCodecSpec {
        key: "flac",
        label: "FLAC",
        // 无损编码，忽略码率参数
        encoder: "flac",
        bitrate: "",
        lossless: true,
    },
];

/// 真编一小段，确认硬件编码器**在这台机器上**能跑。
///
/// 为什么不能只看 `ffmpeg -encoders`：那只是「编译进去了」，运行时仍可能失败。
/// 本机实测就踩到过两种：
/// - `h264_nvenc` / `hevc_nvenc`：`Cannot load nvEncodeAPI64.dll`（驱动没提供，列表里却有）；
/// - `av1_qsv`：`This version of runtime doesn't support AV1 encoding`。
///
/// 素材要够大：QSV 会以「当前分辨率不受支持」拒绝 16×16 这种过小的帧，
/// 那是探测素材的问题、不是硬件不可用，所以这里用 320×240（一帧，几毫秒）。
fn probe_encoder(encoder: &str) -> bool {
    let Some(exe) = sidecar::ffmpeg_path() else {
        return false;
    };
    let mut cmd = Command::new(exe);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=320x240:d=0.1",
        "-frames:v",
        "1",
        "-an",
        "-c:v",
        encoder,
        // 质量参数随便给一个合法值，这里只看能不能编出来
        "-global_quality",
        "23",
        "-f",
        "null",
        "-",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// 一条硬件候选的探测结果
#[derive(Debug, Clone, Copy)]
pub struct HardwareCandidate {
    /// 视频编码 key
    pub key: &'static str,
    pub spec: HardwareCodec,
    /// 本机探测是否通过
    pub available: bool,
}

/// 本机为每个视频编码选定的硬件路径（自动模式用）
#[derive(Debug, Clone, Copy)]
pub struct HardwareChoice {
    /// 视频编码 key
    pub key: &'static str,
    /// 选中的硬件编码器（该编码第一个可用的候选）
    pub spec: HardwareCodec,
}

struct HardwareProbe {
    /// 所有候选及其可用性（界面据此刻画「哪些能选」）
    candidates: Vec<HardwareCandidate>,
    /// 每个编码第一个可用的候选（自动模式用）
    choices: Vec<HardwareChoice>,
}

/// 探测所有硬件候选，进程内只做一次（启动时会后台预热）。
///
/// **串行**探测，且**每条候选都探**（不再「探到一条就停」）：
/// 界面上要把 NVENC 这类候选列出来并标记可用性 —— 只知道「QSV 能用」是不够的，
/// 用户还得看清楚哪条能选、哪条是灰的。
///
/// 为什么不并行探测（踩过一次）：多个硬件会话同时初始化时会互相干扰，
/// 实测出现过「`h264_qsv` 本来可用却探测失败」的假阴性——界面因此白白少给一条
/// 加速路径。串行代价不大（单条 0.3~1.2 秒，只探一次 + 启动预热）。
///
/// 健壮性（用户明确要求）：任何一条候选失败只意味着「这一条不可用」，
/// 全部落空时上层回落软件编码，绝不阻断转码，也不崩。
fn hardware_probe() -> &'static HardwareProbe {
    static CACHE: OnceLock<HardwareProbe> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut candidates: Vec<HardwareCandidate> = Vec::new();
        for codec in VIDEO_CODECS {
            for hw in codec.hardware {
                let mut available = probe_encoder(hw.encoder);
                if !available {
                    // 第一次没探到就隔一会儿再试一次：开机瞬间 GPU 可能正忙
                    // （例如 WebView 正在初始化 WebGL），那属于环境噪音，
                    // 不能当成「这台机器没有这个编码器」上报给用户
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    available = probe_encoder(hw.encoder);
                }
                candidates.push(HardwareCandidate {
                    key: codec.key,
                    spec: *hw,
                    available,
                });
            }
        }

        // 自动模式：每个编码按候选表的优先级取第一个可用的
        let choices = VIDEO_CODECS
            .iter()
            .filter_map(|codec| {
                candidates
                    .iter()
                    .find(|c| c.key == codec.key && c.available)
                    .map(|c| HardwareChoice {
                        key: codec.key,
                        spec: c.spec,
                    })
            })
            .collect();

        HardwareProbe {
            candidates,
            choices,
        }
    })
}

/// 所有硬件候选（含不可用的），界面用它把「能选 / 灰掉」都画出来
pub fn hardware_candidates() -> &'static [HardwareCandidate] {
    &hardware_probe().candidates
}

/// 自动模式下每个编码选定的硬件路径
pub fn hardware_choices() -> &'static [HardwareChoice] {
    &hardware_probe().choices
}

/// 日志/诊断用的可读摘要，如 `["h264:QSV", "h265:NVENC(实验性)"]`
pub fn hardware_summary() -> Vec<String> {
    hardware_choices()
        .iter()
        .map(|c| {
            format!(
                "{}:{}{}",
                c.key,
                c.spec.label,
                if c.spec.experimental { "(实验性)" } else { "" }
            )
        })
        .collect()
}

/// 某个视频编码在本机可用的硬件编码器（自动模式：优先级最高且可用的那条）
pub fn hardware_encoder_for(codec_key: &str) -> Option<HardwareCodec> {
    hardware_choices()
        .iter()
        .find(|c| c.key == codec_key)
        .map(|c| c.spec)
}

/// 用户点名要的那条硬件路径。
///
/// 必须是该编码的候选之一、且**本机探测通过**；否则返回 `None`，
/// 由调用方回落到自动模式（这是「实验性路径不支持也不能出错」的兜底）。
pub fn hardware_encoder_named(codec_key: &str, encoder: &str) -> Option<HardwareCodec> {
    hardware_candidates()
        .iter()
        .find(|c| c.key == codec_key && c.spec.encoder == encoder && c.available)
        .map(|c| c.spec)
}

/// 这条方案实际会走哪条硬件路径（`None` = 软件编码）。
///
/// 界面标注、任务卡文案、参数组装、以及「硬件失败后回落软件重试」全部以它为准，
/// 免得几处各写一份判断、日后走偏。
pub fn effective_hardware(plan: &ConvertPlan) -> Option<HardwareCodec> {
    if plan.mode != ConvertMode::Video || !plan.use_hardware {
        return None;
    }
    let codec = plan.video_codec.as_deref()?;
    // 点名的那条优先；点名的不在候选里或本机不可用，就回落到自动模式
    plan.hardware_encoder
        .as_deref()
        .filter(|name| !name.is_empty())
        .and_then(|name| hardware_encoder_named(codec, name))
        .or_else(|| hardware_encoder_for(codec))
}

/// 这条方案会不会真的走硬件编码
pub fn uses_hardware(plan: &ConvertPlan) -> bool {
    effective_hardware(plan).is_some()
}

/// 视频转换可选的目标容器
pub const CONTAINERS: &[ContainerSpec] = &[
    ContainerSpec {
        key: "mp4",
        label: "MP4",
        muxer: "mp4",
        ext: "mp4",
        video: &["h264", "h265", "av1", "vp9"],
        audio: &["aac", "opus", "mp3", "flac"],
    },
    ContainerSpec {
        key: "mkv",
        label: "MKV",
        muxer: "matroska",
        ext: "mkv",
        video: &["h264", "h265", "av1", "vp9"],
        audio: &["aac", "opus", "mp3", "flac"],
    },
    ContainerSpec {
        key: "webm",
        label: "WebM",
        muxer: "webm",
        ext: "webm",
        // WebM 规范只允许 VP8/VP9/AV1，muxer 会直接拒绝其它视频编码
        video: &["vp9", "av1"],
        // 同理，音频只接受 Opus/Vorbis
        audio: &["opus"],
    },
    ContainerSpec {
        key: "mov",
        label: "MOV",
        muxer: "mov",
        ext: "mov",
        // 「av1 only supported in MP4 and AVIF」「vp9 only supported in MP4」
        video: &["h264", "h265"],
        audio: &["aac", "mp3"],
    },
    ContainerSpec {
        key: "avi",
        label: "AVI",
        muxer: "avi",
        ext: "avi",
        // 实测 avi+h265「写入成功但完整解码报错」，必须排除
        video: &["h264", "av1", "vp9"],
        audio: &["aac", "mp3", "flac"],
    },
    ContainerSpec {
        key: "flv",
        label: "FLV",
        muxer: "flv",
        ext: "flv",
        video: &["h264", "h265", "av1", "vp9"],
        audio: &["aac", "opus", "mp3", "flac"],
    },
];

/// 音频提取的目标：丢视频，只留音轨
pub const AUDIO_TARGETS: &[ContainerSpec] = &[
    ContainerSpec {
        key: "mp3",
        label: "MP3",
        muxer: "mp3",
        ext: "mp3",
        video: &[],
        audio: &["mp3"],
    },
    ContainerSpec {
        key: "m4a",
        label: "M4A (AAC)",
        // m4a 用 ipod muxer，用 mp4 也可以但 iTunes 兼容性差
        muxer: "ipod",
        ext: "m4a",
        video: &[],
        audio: &["aac"],
    },
    ContainerSpec {
        key: "flac",
        label: "FLAC",
        muxer: "flac",
        ext: "flac",
        video: &[],
        audio: &["flac"],
    },
    ContainerSpec {
        key: "wav",
        label: "WAV",
        muxer: "wav",
        ext: "wav",
        video: &[],
        audio: &["pcm"],
    },
];

/// WAV 用的 PCM 编码不参与普通音频编码列表（它只在音频提取里出现）
const PCM_CODEC: AudioCodecSpec = AudioCodecSpec {
    key: "pcm",
    label: "PCM",
    encoder: "pcm_s16le",
    bitrate: "",
    lossless: true,
};

/// 下发给界面的编码选项
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecOption {
    pub key: String,
    pub label: String,
    /// 音频专用：目标码率（kbps）；无损编码与视频编码为空
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    /// 音频专用：是否无损编码（FLAC / PCM）；视频编码恒为 false
    pub lossless: bool,
    /// 视频专用：本机是否有可用的硬件编码器，界面据此标注「GPU」并在下拉里区分
    pub hardware: bool,
    /// 视频专用：选中的硬件编码器短名，如 `QSV` / `NVENC`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardware_label: Option<String>,
    /// 视频专用：这条硬件路径是否实验性（没在开发机验证过，界面要标出来）
    pub hardware_experimental: bool,
    /// 视频专用：可选的编码器路径（自动 / 各硬件候选 / 软件），
    /// 不可用的候选 `available = false`，界面里置灰、选不中
    pub encoders: Vec<EncoderOption>,
}

/// 一条可选的编码器路径
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderOption {
    /// `auto` / `software` / 硬件编码器名（如 `h264_nvenc`）
    pub key: String,
    /// 界面短名：自动 / 软件（CPU）/ QSV / NVENC
    pub label: String,
    /// 本机是否可用；不可用的在界面里置灰
    pub available: bool,
    /// 实验性路径（参数没在开发机验证过）
    pub experimental: bool,
}

/// `-b:a` 的 `192k` → `192`；无损编码的空串得到 `None`
fn bitrate_kbps(raw: &str) -> Option<u32> {
    raw.strip_suffix('k')?.parse().ok()
}

/// 下发给界面的容器选项
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerOption {
    pub key: String,
    pub label: String,
    /// 该容器允许的视频编码 key
    pub video: Vec<String>,
    /// 该容器允许的音频编码 key
    pub audio: Vec<String>,
}

/// 界面下拉用的分辨率选项
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionOption {
    /// `None` 表示保持原始分辨率
    pub height: Option<u32>,
    pub label: String,
}

/// 完整能力表
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertCapabilities {
    pub containers: Vec<ContainerOption>,
    pub audio_targets: Vec<ContainerOption>,
    pub video_codecs: Vec<CodecOption>,
    pub audio_codecs: Vec<CodecOption>,
    pub resolutions: Vec<ResolutionOption>,
    /// 本机是否存在可用的硬件编码器；界面据此决定 GPU 开关是否可用
    pub hardware_available: bool,
}

/// 组装下发给界面的能力表（会真机探测硬件编码器）
pub fn capabilities() -> ConvertCapabilities {
    build_capabilities(hardware_choices())
}

/// 按给定的硬件选型组装能力表。
///
/// 探测与组装分开是为了**可测**：探测结果取决于机器，组装逻辑不该跟着机器变。
/// `hardware` 是「已选定」的路径（自动模式用），`candidates` 是所有候选（含不可用的）。
pub fn build_capabilities(hardware: &[HardwareChoice]) -> ConvertCapabilities {
    build_capabilities_with(hardware, hardware_candidates())
}

fn build_capabilities_with(
    hardware: &[HardwareChoice],
    candidates: &[HardwareCandidate],
) -> ConvertCapabilities {
    let hardware_of = |key: &str| hardware.iter().find(|c| c.key == key).map(|c| c.spec);
    let to_option = |s: &ContainerSpec| ContainerOption {
        key: s.key.to_string(),
        label: s.label.to_string(),
        video: s.video.iter().map(|v| v.to_string()).collect(),
        audio: s.audio.iter().map(|a| a.to_string()).collect(),
    };

    ConvertCapabilities {
        containers: CONTAINERS.iter().map(to_option).collect(),
        audio_targets: AUDIO_TARGETS.iter().map(to_option).collect(),
        video_codecs: VIDEO_CODECS
            .iter()
            .map(|c| {
                let hw = hardware_of(c.key);
                // 自动 / 各硬件候选 / 软件：界面上把这些路径都列出来，
                // 本机用不了的候选标成不可用（置灰、选不中）
                let mut encoders = vec![EncoderOption {
                    key: "auto".to_string(),
                    label: "自动".to_string(),
                    available: true,
                    experimental: false,
                }];
                for cand in candidates.iter().filter(|x| x.key == c.key) {
                    encoders.push(EncoderOption {
                        key: cand.spec.encoder.to_string(),
                        label: cand.spec.label.to_string(),
                        available: cand.available,
                        experimental: cand.spec.experimental,
                    });
                }
                encoders.push(EncoderOption {
                    key: "software".to_string(),
                    label: "软件（CPU）".to_string(),
                    available: true,
                    experimental: false,
                });

                CodecOption {
                    key: c.key.to_string(),
                    label: c.label.to_string(),
                    bitrate_kbps: None,
                    lossless: false,
                    hardware: hw.is_some(),
                    hardware_label: hw.map(|h| h.label.to_string()),
                    // 实验性路径要在界面上标出来：参数没在本机验证过
                    hardware_experimental: hw.map(|h| h.experimental).unwrap_or(false),
                    encoders,
                }
            })
            .collect(),
        // PCM 也要下发：WAV 目标的可选编码就是它，漏了界面会显示成「—」
        audio_codecs: AUDIO_CODECS
            .iter()
            .chain(std::iter::once(&PCM_CODEC))
            .map(|c| CodecOption {
                key: c.key.to_string(),
                label: c.label.to_string(),
                bitrate_kbps: bitrate_kbps(c.bitrate),
                lossless: c.lossless,
                hardware: false,
                hardware_label: None,
                hardware_experimental: false,
                encoders: Vec::new(),
            })
            .collect(),
        // 有硬件候选的编码里，只要有一条探测通过就算「本机有硬件加速」
        hardware_available: VIDEO_CODECS
            .iter()
            .any(|c| !c.hardware.is_empty() && hardware_of(c.key).is_some()),
        resolutions: vec![
            ResolutionOption {
                height: None,
                label: "保持原始分辨率".to_string(),
            },
            ResolutionOption {
                height: Some(1080),
                label: "1080P".to_string(),
            },
            ResolutionOption {
                height: Some(720),
                label: "720P".to_string(),
            },
            ResolutionOption {
                height: Some(480),
                label: "480P".to_string(),
            },
        ],
    }
}

/// 按 key 找容器（视频转换与音频提取一起找）
pub fn container(key: &str) -> Option<&'static ContainerSpec> {
    CONTAINERS
        .iter()
        .chain(AUDIO_TARGETS.iter())
        .find(|c| c.key == key)
}

fn video_codec(key: &str) -> Option<&'static VideoCodecSpec> {
    VIDEO_CODECS.iter().find(|c| c.key == key)
}

fn audio_codec(key: &str) -> Option<&'static AudioCodecSpec> {
    AUDIO_CODECS
        .iter()
        .chain(std::iter::once(&PCM_CODEC))
        .find(|c| c.key == key)
}

/// 校验一个转码方案是否真的可执行。
///
/// 这里是**最后一道闸**：界面虽然做了联动过滤，但任务要落盘、能在重启后恢复，
/// 校验必须放在后端而不是只信前端传来的值。
pub fn validate_plan(plan: &ConvertPlan) -> AppResult<()> {
    let spec = container(&plan.container).ok_or_else(|| {
        AppError::new(
            "MEDIA_UNSUPPORTED_TARGET",
            format!("不支持的目标格式：{}", plan.container),
            false,
        )
    })?;

    if plan.mode == ConvertMode::AudioExtract {
        if !spec.video.is_empty() {
            return Err(AppError::new(
                "MEDIA_UNSUPPORTED_TARGET",
                format!("{} 不是音频提取格式", spec.label),
                false,
            ));
        }
        // 提取模式只允许一个音轨编码，忽略传入的视频编码
        let audio = plan.audio_codec.as_deref().unwrap_or("");
        if !spec.audio.contains(&audio) {
            return Err(AppError::new(
                "MEDIA_UNSUPPORTED_TARGET",
                format!("{} 不支持该音频编码", spec.label),
                false,
            ));
        }
        return Ok(());
    }

    if spec.video.is_empty() {
        return Err(AppError::new(
            "MEDIA_UNSUPPORTED_TARGET",
            format!("{} 只能用于音频提取", spec.label),
            false,
        ));
    }

    let video = plan.video_codec.as_deref().unwrap_or("");
    if video_codec(video).is_none() {
        return Err(AppError::new(
            "MEDIA_UNSUPPORTED_TARGET",
            format!("不支持的视频编码：{video}"),
            false,
        ));
    }
    if !spec.video.contains(&video) {
        return Err(AppError::new(
            "MEDIA_INCOMPATIBLE_CODEC",
            format!("{} 容器不支持 {} 视频编码", spec.label, video_label(video)),
            false,
        )
        .with_hint("请改选容器支持的编码，或换一个容器"));
    }

    let audio = plan.audio_codec.as_deref().unwrap_or("");
    if audio_codec(audio).is_none() {
        return Err(AppError::new(
            "MEDIA_UNSUPPORTED_TARGET",
            format!("不支持的音频编码：{audio}"),
            false,
        ));
    }
    if !spec.audio.contains(&audio) {
        return Err(AppError::new(
            "MEDIA_INCOMPATIBLE_CODEC",
            format!("{} 容器不支持 {} 音频编码", spec.label, audio_label(audio)),
            false,
        )
        .with_hint("请改选容器支持的音频编码，或换一个容器"));
    }

    Ok(())
}

fn video_label(key: &str) -> &str {
    video_codec(key).map(|c| c.label).unwrap_or(key)
}

fn audio_label(key: &str) -> &str {
    audio_codec(key).map(|c| c.label).unwrap_or(key)
}

/// 组装 FFmpeg 参数。
///
/// 调用方负责在前面补上 `-hide_banner -loglevel error -progress pipe:1 -nostats -y`
/// 与 `-i <输入>`（见 `media::transcode`）。
///
/// `plan.use_hardware` 为真时优先走硬件编码器，但它只是「允许」：
/// 该编码必须真有硬件路径、且本机探测通过，否则静默回落软件编码——
/// 宁可慢，也不能因为用户开了开关就产出一个编不出来的任务。
pub fn ffmpeg_args(plan: &ConvertPlan, out: &Path, threads: u32) -> AppResult<Vec<String>> {
    ffmpeg_args_impl(plan, out, threads, plan.use_hardware)
}

/// 强制软件编码的参数，供「硬件路径编不动时重跑一次」使用。
pub fn software_ffmpeg_args(plan: &ConvertPlan, out: &Path, threads: u32) -> AppResult<Vec<String>> {
    ffmpeg_args_impl(plan, out, threads, false)
}

fn ffmpeg_args_impl(
    plan: &ConvertPlan,
    out: &Path,
    threads: u32,
    allow_hardware: bool,
) -> AppResult<Vec<String>> {
    validate_plan(plan)?;

    let spec = container(&plan.container).expect("validate_plan 已确认容器存在");
    let mut args: Vec<String> = Vec::new();

    if plan.mode == ConvertMode::AudioExtract {
        // 丢视频，只留音轨
        args.push("-vn".into());
        let audio = audio_codec(plan.audio_codec.as_deref().unwrap_or(""))
            .expect("validate_plan 已确认编码存在");
        args.push("-c:a".into());
        args.push(audio.encoder.into());
        if !audio.bitrate.is_empty() {
            args.push("-b:a".into());
            args.push(audio.bitrate.into());
        }
    } else {
        let video = video_codec(plan.video_codec.as_deref().unwrap_or(""))
            .expect("validate_plan 已确认编码存在");
        // 硬件路径：方案里点名了就走点名的那条（不可用则回落自动），
        // 没点名就走自动；`allow_hardware=false`（软件重试）时一律不走
        let hardware = if allow_hardware {
            effective_hardware(plan)
        } else {
            None
        };
        match hardware {
            Some(hw) => {
                args.push("-c:v".into());
                args.push(hw.encoder.into());
                args.extend(hw.quality.iter().map(|s| s.to_string()));
                // 不给 -threads：硬件编码器自己管并发，这个参数对它们没有意义，
                // 而且软件路径的限流初衷（别把 CPU 吃满）对 GPU 编码根本不适用
            }
            None => {
                args.push("-c:v".into());
                args.push(video.encoder.into());
                args.extend(video.quality.iter().map(|s| s.to_string()));
                // 转码吃满 CPU 会把界面拖卡（下载任务也在同一台机器上跑），限一下线程
                if threads > 0 {
                    args.push("-threads".into());
                    args.push(threads.to_string());
                }
            }
        }
        // 偶数尺寸：多数编码器要求宽高是 2 的倍数，-2 让 FFmpeg 自己取整
        if let Some(h) = plan.scale_height.filter(|h| *h > 0) {
            args.push("-vf".into());
            args.push(format!("scale=-2:{h}"));
        }

        let audio = audio_codec(plan.audio_codec.as_deref().unwrap_or(""))
            .expect("validate_plan 已确认编码存在");
        args.push("-c:a".into());
        args.push(audio.encoder.into());
        if !audio.bitrate.is_empty() {
            args.push("-b:a".into());
            args.push(audio.bitrate.into());
        }
    }

    // faststart 把索引挪到文件头部，只有 mp4/mov 系支持；其余容器加了会报错
    if matches!(spec.muxer, "mp4" | "mov" | "ipod") {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }

    // 输出是 `.part` 结尾，FFmpeg 推不出容器格式，必须显式指定
    args.push("-f".into());
    args.push(spec.muxer.into());
    args.push(out.to_string_lossy().to_string());

    Ok(args)
}

/// 目标产物的扩展名
pub fn extension_of(container_key: &str) -> Option<&'static str> {
    container(container_key).map(|c| c.ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(mode: ConvertMode, container: &str, video: &str, audio: &str) -> ConvertPlan {
        ConvertPlan {
            source_path: "D:\\in.mp4".into(),
            mode,
            container: container.into(),
            video_codec: Some(video.into()),
            audio_codec: Some(audio.into()),
            scale_height: None,
            use_hardware: false,
            hardware_encoder: None,
        }
    }

    #[test]
    fn 常规组合通过校验() {
        for (container, video, audio) in [
            ("mp4", "h264", "aac"),
            ("mp4", "h265", "flac"),
            ("mkv", "av1", "opus"),
            ("webm", "vp9", "opus"),
            ("webm", "av1", "opus"),
            ("mov", "h264", "mp3"),
            ("avi", "h264", "mp3"),
            ("flv", "h264", "aac"),
        ] {
            let p = plan(ConvertMode::Video, container, video, audio);
            assert!(
                validate_plan(&p).is_ok(),
                "{container}+{video}+{audio} 应当被接受"
            );
        }
    }

    #[test]
    fn avi_配_h265_被拒绝() {
        // 这是最阴的一种：FFmpeg 写入成功、不报任何错，但产物完整解码会失败。
        // 只能在这里拦住，否则用户拿到一个打不开的文件却不知道原因。
        let p = plan(ConvertMode::Video, "avi", "h265", "aac");
        let err = validate_plan(&p).unwrap_err();
        assert_eq!(err.code, "MEDIA_INCOMPATIBLE_CODEC");
    }

    #[test]
    fn webm_只接受_vp9_或_av1_加_opus() {
        // 视频：h264/h265 会被 webm muxer 直接拒绝
        for bad in ["h264", "h265"] {
            let p = plan(ConvertMode::Video, "webm", bad, "opus");
            assert_eq!(
                validate_plan(&p).unwrap_err().code,
                "MEDIA_INCOMPATIBLE_CODEC",
                "webm+{bad} 应被拒"
            );
        }
        // 音频：只接受 opus
        for bad in ["aac", "mp3", "flac"] {
            let p = plan(ConvertMode::Video, "webm", "vp9", bad);
            assert_eq!(
                validate_plan(&p).unwrap_err().code,
                "MEDIA_INCOMPATIBLE_CODEC",
                "webm+{bad} 应被拒"
            );
        }
    }

    #[test]
    fn mov_只接受_h264_h265_加_aac_mp3() {
        for bad in ["av1", "vp9"] {
            let p = plan(ConvertMode::Video, "mov", bad, "aac");
            assert_eq!(validate_plan(&p).unwrap_err().code, "MEDIA_INCOMPATIBLE_CODEC");
        }
        for bad in ["opus", "flac"] {
            let p = plan(ConvertMode::Video, "mov", "h264", bad);
            assert_eq!(validate_plan(&p).unwrap_err().code, "MEDIA_INCOMPATIBLE_CODEC");
        }
    }

    #[test]
    fn 音频提取只接受四种目标() {
        for target in ["mp3", "m4a", "flac", "wav"] {
            let spec = container(target).expect("音频目标应存在");
            let audio = spec.audio[0];
            let p = plan(ConvertMode::AudioExtract, target, "", audio);
            assert!(validate_plan(&p).is_ok(), "{target} 音频提取应被接受");
        }
        // 视频容器不能用于音频提取
        let p = plan(ConvertMode::AudioExtract, "mp4", "", "aac");
        assert_eq!(validate_plan(&p).unwrap_err().code, "MEDIA_UNSUPPORTED_TARGET");
    }

    #[test]
    fn 音频容器不能用于视频转换() {
        let p = plan(ConvertMode::Video, "mp3", "h264", "mp3");
        let err = validate_plan(&p).unwrap_err();
        assert_eq!(err.code, "MEDIA_UNSUPPORTED_TARGET");
        assert!(err.message.contains("只能用于音频提取"));
    }

    #[test]
    fn 未知容器与未知编码被拒绝() {
        let p = plan(ConvertMode::Video, "rmvb", "h264", "aac");
        assert_eq!(validate_plan(&p).unwrap_err().code, "MEDIA_UNSUPPORTED_TARGET");

        let p = plan(ConvertMode::Video, "mp4", "wmv3", "aac");
        assert_eq!(validate_plan(&p).unwrap_err().code, "MEDIA_UNSUPPORTED_TARGET");

        let p = plan(ConvertMode::Video, "mp4", "h264", "vorbis");
        assert_eq!(validate_plan(&p).unwrap_err().code, "MEDIA_UNSUPPORTED_TARGET");
    }

    #[test]
    fn 参数里带显式容器格式() {
        // 输出文件是 `.part` 结尾，不指定 -f 会让 FFmpeg 推不出格式而失败
        let p = plan(ConvertMode::Video, "mkv", "h265", "aac");
        let args = ffmpeg_args(&p, Path::new("D:\\out.mkv.part"), 4).unwrap();
        let f_pos = args.iter().position(|a| a == "-f").expect("必须有 -f");
        assert_eq!(args[f_pos + 1], "matroska");
        assert_eq!(args.last().unwrap(), "D:\\out.mkv.part");
    }

    #[test]
    fn 只有_mp4_系容器加_faststart() {
        let with = ffmpeg_args(
            &plan(ConvertMode::Video, "mp4", "h264", "aac"),
            Path::new("o.part"),
            0,
        )
        .unwrap();
        assert!(with.iter().any(|a| a == "+faststart"), "mp4 应当加 faststart");

        let without = ffmpeg_args(
            &plan(ConvertMode::Video, "webm", "vp9", "opus"),
            Path::new("o.part"),
            0,
        )
        .unwrap();
        assert!(
            !without.iter().any(|a| a == "+faststart"),
            "webm 加 faststart 会失败"
        );
    }

    #[test]
    fn 音频提取参数丢掉视频() {
        let p = plan(ConvertMode::AudioExtract, "mp3", "", "mp3");
        let args = ffmpeg_args(&p, Path::new("o.part"), 0).unwrap();
        assert!(args.iter().any(|a| a == "-vn"), "音频提取必须 -vn");
        assert!(!args.iter().any(|a| a == "-c:v"), "不应出现视频编码参数");
        let f_pos = args.iter().position(|a| a == "-f").unwrap();
        assert_eq!(args[f_pos + 1], "mp3");
    }

    #[test]
    fn 缩放参数使用_scale_滤镜() {
        let mut p = plan(ConvertMode::Video, "mp4", "h264", "aac");
        p.scale_height = Some(720);
        let args = ffmpeg_args(&p, Path::new("o.part"), 0).unwrap();
        assert!(args.iter().any(|a| a == "scale=-2:720"), "应生成缩放滤镜");
    }

    #[test]
    fn 线程数只在本机限流时下发() {
        let p = plan(ConvertMode::Video, "mp4", "h264", "aac");
        let limited = ffmpeg_args(&p, Path::new("o.part"), 2).unwrap();
        let pos = limited.iter().position(|a| a == "-threads").expect("应有限流");
        assert_eq!(limited[pos + 1], "2");

        // 0 表示交给 FFmpeg 自己决定，不下发参数
        let auto = ffmpeg_args(&p, Path::new("o.part"), 0).unwrap();
        assert!(!auto.iter().any(|a| a == "-threads"));
    }

    #[test]
    fn 扩展名映射覆盖全部容器() {
        assert_eq!(extension_of("mp4"), Some("mp4"));
        assert_eq!(extension_of("mkv"), Some("mkv"));
        assert_eq!(extension_of("m4a"), Some("m4a"));
        assert_eq!(extension_of("wav"), Some("wav"));
        assert_eq!(extension_of("rmvb"), None);
    }

    #[test]
    fn 开启硬件加速时按探测结果选择编码器() {
        let mut p = plan(ConvertMode::Video, "mp4", "h264", "aac");
        p.use_hardware = true;
        let args = ffmpeg_args(&p, Path::new("o.part"), 4).unwrap();
        match hardware_encoder_for("h264") {
            Some(hw) => {
                let pos = args
                    .iter()
                    .position(|a| a == hw.encoder)
                    .unwrap_or_else(|| panic!("探测通过时应走硬件 {}", hw.encoder));
                // 紧跟编码器之后的必须正好是这条路径自己的质量参数：
                // 喂错参数 FFmpeg 不报错，只会静默忽略 —— 只能在这里锁死
                let after: Vec<&str> = args[pos + 1..]
                    .iter()
                    .take(hw.quality.len())
                    .map(|s| s.as_str())
                    .collect();
                assert_eq!(after, hw.quality.to_vec(), "硬件质量参数串了路径");
                assert!(!args.iter().any(|a| a == "-threads"), "硬件路径不下发线程限流");
            }
            None => assert!(args.iter().any(|a| a == "libx264"), "没有硬件时要回落软件"),
        }
    }

    #[test]
    fn 点名硬件路径不可用时会回落而不是失败() {
        // 点名一条本机不可用的路径（NVENC 在这台机器上必然探测失败）：
        // 不能报错、不能把坏参数交给 FFmpeg，只能落回自动或软件
        let mut p = plan(ConvertMode::Video, "mp4", "h264", "aac");
        p.use_hardware = true;
        p.hardware_encoder = Some("h264_nvenc".to_string());
        let args = ffmpeg_args(&p, Path::new("o.part"), 0).unwrap();
        let used_nvenc = args.iter().any(|a| a == "h264_nvenc");
        let nvenc_ok = hardware_encoder_named("h264", "h264_nvenc").is_some();
        assert_eq!(used_nvenc, nvenc_ok, "点名路径的可用性必须与探测结果一致");
        if !nvenc_ok {
            // 回落：要么自动选了 QSV，要么干脆软件编码
            assert!(
                args.iter().any(|a| a == "h264_qsv" || a == "libx264"),
                "点名路径不可用时应回落自动/软件"
            );
        }

        // 名字写错（候选表里根本没有）同样只是回落，不报错
        let mut p2 = plan(ConvertMode::Video, "mp4", "h264", "aac");
        p2.use_hardware = true;
        p2.hardware_encoder = Some("h264_不存在".to_string());
        assert!(ffmpeg_args(&p2, Path::new("o.part"), 0).is_ok());
    }

    #[test]
    fn 能力表列出每条编码器路径与可用性() {
        let caps = build_capabilities(&[HardwareChoice {
            key: "h264",
            spec: H264_HARDWARE[0],
        }]);
        let h264 = caps
            .video_codecs
            .iter()
            .find(|c| c.key == "h264")
            .expect("h264 应在表里");
        let keys: Vec<&str> = h264.encoders.iter().map(|e| e.key.as_str()).collect();
        // 自动 + 两个硬件候选（QSV / NVENC）+ 软件
        assert_eq!(keys, vec!["auto", "h264_qsv", "h264_nvenc", "software"]);
        let nvenc = h264
            .encoders
            .iter()
            .find(|e| e.key == "h264_nvenc")
            .expect("NVENC 候选要下发到界面");
        assert!(nvenc.experimental, "实验性要标出来");
        // 可用性来自真实探测：本机 NVENC 不可用时必须是 false（界面据此置灰）
        assert_eq!(
            nvenc.available,
            hardware_encoder_named("h264", "h264_nvenc").is_some()
        );
        assert!(h264.encoders.iter().find(|e| e.key == "software").unwrap().available);
        // AV1 没有硬件候选，只剩自动 + 软件
        let av1 = caps.video_codecs.iter().find(|c| c.key == "av1").unwrap();
        let keys: Vec<&str> = av1.encoders.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["auto", "software"]);
    }

    #[test]
    fn 硬件候选按优先级排列且参数互不套用() {
        // 本机验证过的 QSV 排第一，实验性的 NVENC 排第二
        assert_eq!(H264_HARDWARE[0].encoder, "h264_qsv");
        assert_eq!(H265_HARDWARE[0].encoder, "hevc_qsv");

        let qsv = H264_HARDWARE[0];
        let nv = H264_HARDWARE[1];
        assert_eq!(nv.encoder, "h264_nvenc");
        assert!(qsv.quality.contains(&"-global_quality"));
        assert!(!nv.quality.contains(&"-global_quality"), "NVENC 不能套 QSV 的参数");
        assert!(nv.quality.contains(&"-cq"), "NVENC 用 cq 做恒定质量");
        assert!(!qsv.experimental, "验证过的路径不标实验性");
        assert!(nv.experimental, "没在本机验证过的路径必须标实验性");

        // 同一个编码最多选中一条，且必须是候选表里的成员
        for choice in hardware_choices() {
            let spec = video_codec(choice.key).expect("选中的编码应在表里");
            assert!(
                spec.hardware.iter().any(|h| h.encoder == choice.spec.encoder),
                "{} 选中了候选表外的编码器",
                choice.key
            );
        }
        let keys: Vec<&str> = hardware_choices().iter().map(|c| c.key).collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(keys.len(), unique.len(), "同一个编码只能有一条硬件路径");
    }

    #[test]
    fn 探测失败时静默回落软件而不是报错() {
        // 用一个必然失败的编码器名模拟「驱动缺失 / 参数不被支持」：
        // 探测必须老老实实返回 false（而不是 panic 或误判通过）
        assert!(!probe_encoder("h264_definitely_not_a_real_encoder"));

        // 探测失败 → 候选落空 → 参数走软件
        let mut p = plan(ConvertMode::Video, "mp4", "av1", "aac");
        p.use_hardware = true; // AV1 没有硬件候选，开关打开也只能软件
        let args = ffmpeg_args(&p, Path::new("o.part"), 0).unwrap();
        assert!(args.iter().any(|a| a == "libaom-av1"));

        // 强制软件参数的入口必须与探测结果无关
        let soft = software_ffmpeg_args(
            &plan(ConvertMode::Video, "mp4", "h264", "aac"),
            Path::new("o.part"),
            0,
        )
        .unwrap();
        assert!(soft.iter().any(|a| a == "libx264"));
        assert!(
            !soft.iter().any(|a| a.ends_with("_qsv") || a.ends_with("_nvenc")),
            "强制软件时不能出现任何硬件编码器"
        );
    }

    #[test]
    fn 关闭硬件加速时一律走软件() {
        let p = plan(ConvertMode::Video, "mp4", "h264", "aac");
        let args = ffmpeg_args(&p, Path::new("o.part"), 0).unwrap();
        assert!(args.iter().any(|a| a == "libx264"));
        assert!(!args.iter().any(|a| a.ends_with("_qsv")), "开关关闭时不该出现硬件编码器");
    }

    #[test]
    fn av1_与_vp9_没有硬件路径() {
        // 实测 av1_qsv / vp9_qsv 会被 QSV 运行时直接拒绝，所以即使开了开关也只能软件编
        for (container, key) in [("mp4", "av1"), ("mp4", "vp9")] {
            let mut p = plan(ConvertMode::Video, container, key, "aac");
            p.use_hardware = true;
            let args = ffmpeg_args(&p, Path::new("o.part"), 0).unwrap();
            let expect = video_codec(key).unwrap().encoder;
            assert!(
                args.iter().any(|a| a == expect),
                "{key} 应回落软件编码 {expect}"
            );
        }
    }

    #[test]
    fn 能力表标注硬件可用性与音频档位() {
        // 返回值带借用关系，写成函数而不是闭包（闭包的生命周期标注会被推断成另一个）
        fn video<'a>(caps: &'a ConvertCapabilities, key: &str) -> &'a CodecOption {
            caps.video_codecs
                .iter()
                .find(|c| c.key == key)
                .expect("视频编码应在表里")
        }
        fn audio<'a>(caps: &'a ConvertCapabilities, key: &str) -> &'a CodecOption {
            caps.audio_codecs
                .iter()
                .find(|c| c.key == key)
                .expect("音频编码应在表里")
        }

        let with = build_capabilities(&[HardwareChoice {
            key: "h264",
            spec: H264_HARDWARE[0],
        }]);
        assert!(with.hardware_available);
        assert!(video(&with, "h264").hardware, "探测到 h264 时应标注可用");
        assert_eq!(
            video(&with, "h264").hardware_label.as_deref(),
            Some("QSV"),
            "已有验证的路径不加「实验性」"
        );
        assert!(!video(&with, "h265").hardware, "没探测到的编码不能标");
        assert!(video(&with, "h265").hardware_label.is_none());
        assert!(!video(&with, "av1").hardware, "AV1 没有硬件路径");

        // 实验性候选（NVENC）要标出来，用户才知道这条路没在本机验证过
        let nv = build_capabilities(&[HardwareChoice {
            key: "h264",
            spec: H264_HARDWARE[1],
        }]);
        assert_eq!(video(&nv, "h264").hardware_label.as_deref(), Some("NVENC"));
        assert!(video(&nv, "h264").hardware_experimental, "实验性要能传到界面");
        assert!(!video(&with, "h264").hardware_experimental);
        assert!(nv.hardware_available);

        let without = build_capabilities(&[]);
        assert!(!without.hardware_available);
        assert!(without.video_codecs.iter().all(|c| !c.hardware));

        let flac = audio(&with, "flac");
        assert!(flac.lossless);
        assert_eq!(flac.bitrate_kbps, None, "无损编码没有目标码率");
        let mp3 = audio(&with, "mp3");
        assert!(!mp3.lossless);
        assert_eq!(mp3.bitrate_kbps, Some(192));
        // WAV 目标的可选编码是 PCM，不下发界面就只能显示「—」
        assert_eq!(audio(&with, "pcm").label, "PCM");
    }

    #[test]
    fn 码率文本解析成_kbps() {
        assert_eq!(bitrate_kbps("192k"), Some(192));
        assert_eq!(bitrate_kbps("160k"), Some(160));
        assert_eq!(bitrate_kbps(""), None, "无损编码是空串");
        assert_eq!(bitrate_kbps("192"), None, "缺单位说明参数本身写错了");
    }

    #[test]
    fn 下发的能力表与校验规则一致() {
        // 界面完全依赖这张表做联动过滤，一旦它和 validate_plan 走偏，
        // 用户就会从下拉里选到一个提交必失败的组合
        let caps = build_capabilities(&[]);
        for c in &caps.containers {
            let spec = container(&c.key).unwrap();
            assert_eq!(c.video, spec.video, "{} 的视频编码列表不一致", c.key);
            assert_eq!(c.audio, spec.audio, "{} 的音频编码列表不一致", c.key);
            // 表里列出的每个编码都必须真的能通过校验
            for v in &c.video {
                for a in &c.audio {
                    let p = plan(ConvertMode::Video, &c.key, v, a);
                    assert!(
                        validate_plan(&p).is_ok(),
                        "能力表声称 {}+{v}+{a} 可用，但校验不通过",
                        c.key
                    );
                }
            }
        }
        for c in &caps.audio_targets {
            for a in &c.audio {
                let p = plan(ConvertMode::AudioExtract, &c.key, "", a);
                assert!(
                    validate_plan(&p).is_ok(),
                    "能力表声称音频提取 {}+{a} 可用，但校验不通过",
                    c.key
                );
            }
        }
    }
}