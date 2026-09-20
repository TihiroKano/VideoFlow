/**
 * 音频转换质量提示规则测试。
 *
 * 运行：node --test test/unit/
 */

import { strict as assert } from "node:assert";
import test from "node:test";

import {
  audioQualityHints,
  isLosslessSource,
  type AudioTarget,
} from "../../src/features/convert/audioQuality.ts";

const MP3: AudioTarget = { key: "mp3", label: "MP3", lossless: false, bitrateKbps: 192 };
const AAC: AudioTarget = { key: "aac", label: "AAC", lossless: false, bitrateKbps: 192 };
const OPUS: AudioTarget = { key: "opus", label: "Opus", lossless: false, bitrateKbps: 160 };
const FLAC: AudioTarget = { key: "flac", label: "FLAC", lossless: true };
const WAV: AudioTarget = { key: "wav", label: "WAV", lossless: true };

test("无损名单认得 PCM 一族与容错大小写", () => {
  assert.equal(isLosslessSource("flac"), true);
  assert.equal(isLosslessSource("FLAC"), true);
  assert.equal(isLosslessSource("pcm_s16le"), true);
  assert.equal(isLosslessSource("alac"), true);
  assert.equal(isLosslessSource("aac"), false);
  assert.equal(isLosslessSource("mp3"), false);
  assert.equal(isLosslessSource(null), false);
  assert.equal(isLosslessSource(undefined), false);
});

test("无损转有损是正常压缩，不提示", () => {
  // FLAC → MP3 正是用户想要的「高质量转低质量」，不该拦
  const hints = audioQualityHints([{ name: "a.flac", codec: "flac", bitrate: 900000 }], MP3);
  assert.equal(hints.length, 0);
});

test("有损转无损提醒不会提升音质", () => {
  const hints = audioQualityHints([{ name: "a.mp3", codec: "mp3", bitrate: 192000 }], FLAC);
  assert.equal(hints.length, 1);
  assert.equal(hints[0]?.level, "warning");
  assert.match(hints[0]?.text ?? "", /不会找回已经丢掉的细节/);
  assert.match(hints[0]?.text ?? "", /a\.mp3/);
});

test("有损转有损且目标码率不低时提醒音质不会变好", () => {
  // 128k AAC → 192k MP3：码率更高，但音质不会变好
  const hints = audioQualityHints([{ name: "a.m4a", codec: "aac", bitrate: 128000 }], MP3);
  assert.equal(hints.length, 1);
  assert.equal(hints[0]?.level, "warning");
  assert.match(hints[0]?.text ?? "", /音质不会变好/);
  assert.match(hints[0]?.text ?? "", /192 kbps/);
});

test("有损转有损但码率下降时不提示", () => {
  const hints = audioQualityHints([{ name: "a.mp3", codec: "mp3", bitrate: 320000 }], MP3);
  assert.equal(hints.length, 0);
});

test("码率读不到时不猜，也不提示", () => {
  // flac 的流级码率是 N/A，退到容器级也可能失败；这时不能编一个数字下结论
  const hints = audioQualityHints([{ name: "a.mp3", codec: "mp3", bitrate: null }], MP3);
  assert.equal(hints.length, 0);
});

test("无损转无损只做说明", () => {
  const hints = audioQualityHints([{ name: "a.flac", codec: "flac", bitrate: 900000 }], WAV);
  assert.equal(hints.length, 1);
  assert.equal(hints[0]?.level, "info");
  assert.match(hints[0]?.text ?? "", /不会改变音质/);
  assert.match(hints[0]?.text ?? "", /WAV 体积会大很多/);
});

test("多个文件按规则归并成一条", () => {
  const hints = audioQualityHints(
    [
      { name: "a.mp3", codec: "mp3", bitrate: 192000 },
      { name: "b.m4a", codec: "aac", bitrate: 128000 },
      { name: "c.mp3", codec: "mp3", bitrate: 256000 },
      { name: "d.mp3", codec: "mp3", bitrate: 320000 },
    ],
    FLAC,
  );
  assert.equal(hints.length, 1, "同一类问题的文件要合成一条，而不是刷满屏幕");
  assert.match(hints[0]?.text ?? "", /等 4 个文件/);
});

test("无损与有损混在一起时两条提示各说各的", () => {
  const hints = audioQualityHints(
    [
      { name: "a.flac", codec: "flac", bitrate: 900000 },
      { name: "b.mp3", codec: "mp3", bitrate: 192000 },
    ],
    FLAC,
  );
  const levels = hints.map((h) => h.level).sort();
  assert.deepEqual(levels, ["info", "warning"]);
});

test("有损目标时无损文件不计入码率比较", () => {
  // 同一个列表里 flac 走正常压缩，mp3 因为码率不降被提醒
  const hints = audioQualityHints(
    [
      { name: "a.flac", codec: "flac", bitrate: 900000 },
      { name: "b.mp3", codec: "mp3", bitrate: 128000 },
    ],
    OPUS,
  );
  assert.equal(hints.length, 1);
  assert.match(hints[0]?.text ?? "", /b\.mp3/);
  assert.doesNotMatch(hints[0]?.text ?? "", /a\.flac/);
});

test("空文件列表与无音轨都不提示", () => {
  assert.equal(audioQualityHints([], FLAC).length, 0);
  assert.equal(
    audioQualityHints([{ name: "a.bin", codec: null, bitrate: null }], AAC).length,
    0,
  );
});