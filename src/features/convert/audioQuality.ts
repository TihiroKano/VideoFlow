/**
 * 音频转换的质量提示规则。
 *
 * 转码这件事本身不会「变好」，只会「保住」或「损失」。用户最容易误解的两种：
 * 1. 把有损音频转成无损（MP3 → FLAC）：**丢掉的细节找不回来**，只是文件变大；
 * 2. 有损转有损但目标码率不低于源（128k AAC → 192k MP3）：**音质不会变好**，
 *    更高的码率只是不再继续损失而已。
 *
 * 而「无损 → 有损」（FLAC → MP3）是正常压缩，不该弹提示吓唬人。
 *
 * 这里刻意做成纯函数：规则边界（无损名单、码率读不到、多文件归并）都值得单测，
 * 见 `test/unit/audioQuality.test.ts`。
 */

/** 源文件的音频信息（只需要判断质量这几项） */
export interface AudioSource {
  /** 文件名，用于提示里说明是哪个文件 */
  name: string;
  /** ffprobe 的 codec_name，如 aac / flac / pcm_s16le */
  codec?: string | null;
  /** 音频码率（bps）；读不到时为 null / undefined */
  bitrate?: number | null;
}

/** 目标音频编码（来自主进程下发的能力表） */
export interface AudioTarget {
  key: string;
  label: string;
  /** 是否无损编码（FLAC / PCM） */
  lossless: boolean;
  /** 目标码率（kbps）；无损编码没有 */
  bitrateKbps?: number;
}

export interface QualityHint {
  /** warning：转了也不会变好，甚至更糟；info：只是说明性质 */
  level: "warning" | "info";
  text: string;
}

/** 无损音频编码（ffprobe 的 codec_name 口径） */
const LOSSLESS_CODECS = new Set(["flac", "alac", "ape", "wavpack", "tta", "truehd", "mlp", "tak"]);

/** 源编码是不是无损；PCM 一族名字带前缀（pcm_s16le 等） */
export function isLosslessSource(codec?: string | null): boolean {
  if (!codec) return false;
  const c = codec.trim().toLowerCase();
  return LOSSLESS_CODECS.has(c) || c.startsWith("pcm");
}

/** 文件列到提示里：太多时只列前三个 */
function fileNames(files: string[]): string {
  if (files.length <= 3) return files.join("、");
  return `${files.slice(0, 3).join("、")} 等 ${files.length} 个文件`;
}

function uniqueLabels(codecs: string[]): string {
  const upper = [...new Set(codecs.map((c) => c.toUpperCase()))];
  return upper.join(" / ");
}

/**
 * 按规则汇总提示。
 *
 * - 无损 → 有损：不提示（这就是正常压缩，用户在做什么很清楚）；
 * - 无损 → 无损：说明音质不变，只是换封装（体积可能暴涨）；
 * - 有损 → 无损：warning，找不回细节且文件更大；
 * - 有损 → 有损：目标码率不低于源时 warning，音质不会变好。
 *
 * 码率读不到的文件不参与第三条判断：宁可不说，也不能拿编出来的数字下结论。
 */
export function audioQualityHints(sources: AudioSource[], target: AudioTarget): QualityHint[] {
  const hints: QualityHint[] = [];

  const toLossless = sources.filter((s) => !isLosslessSource(s.codec));
  const fromLossless = sources.filter((s) => isLosslessSource(s.codec));
  const sameCodec = new Set(sources.map((s) => (s.codec ?? "").toLowerCase())).size === 1;

  if (target.lossless) {
    if (fromLossless.length > 0) {
      hints.push({
        level: "info",
        text:
          `无损转无损（${uniqueLabels(fromLossless.map((s) => s.codec ?? "无损"))} → ${target.label}）` +
          `不会改变音质，只是换封装；${target.key === "wav" ? "WAV 体积会大很多，" : ""}听感完全一致。` +
          `（${fileNames(fromLossless.map((s) => s.name))}）`,
      });
    }
    if (toLossless.length > 0) {
      hints.push({
        level: "warning",
        text:
          `「${target.label}」是无损格式，但源是有损编码` +
          `${sameCodec && toLossless[0]?.codec ? `（${toLossless[0].codec.toUpperCase()}）` : ""}：` +
          `转成无损不会找回已经丢掉的细节，音质和原来一样，文件还会明显变大。` +
          `（${fileNames(toLossless.map((s) => s.name))}）`,
      });
    }
    return hints;
  }

  // 有损目标：无损源是正常压缩，不提示；有损源只在「码率不降」时才提醒
  const notLower = sources.filter((s) => {
    if (isLosslessSource(s.codec)) return false;
    if (!target.bitrateKbps || !s.bitrate) return false;
    return target.bitrateKbps * 1000 >= s.bitrate;
  });
  if (notLower.length > 0 && target.bitrateKbps) {
    const sample = notLower[0];
    const sourceKbps = sample?.bitrate ? Math.round(sample.bitrate / 1000) : 0;
    hints.push({
      level: "warning",
      text:
        `目标「${target.label}」的码率 ${target.bitrateKbps} kbps 不低于源` +
        `${sameCodec && sourceKbps ? `（${sourceKbps} kbps）` : ""}：` +
        `音质不会变好，只是不再继续损失；想变小就选更低的码率。` +
        `（${fileNames(notLower.map((s) => s.name))}）`,
    });
  }

  return hints;
}