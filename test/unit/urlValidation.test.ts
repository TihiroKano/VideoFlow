/**
 * URL 校验纯函数测试。
 *
 * 运行：node --test test/unit/
 * （Node 24 原生支持 TypeScript 类型擦除，无需额外依赖）
 */

import { strict as assert } from "node:assert";
import test from "node:test";

import { checkUrl, isPrivateHost } from "../../src/features/compose/urlValidation.ts";

test("空输入被拒绝", () => {
  const r = checkUrl("   ");
  assert.equal(r.ok, false);
  assert.equal(r.issue?.code, "URL_EMPTY");
});

test("缺少 scheme 时自动补全 https", () => {
  const r = checkUrl("www.bilibili.com/video/BV1xx");
  assert.equal(r.ok, true);
  assert.ok(r.normalized.startsWith("https://"));
});

test("http 链接被拒绝", () => {
  const r = checkUrl("http://example.com/a.mp4");
  assert.equal(r.ok, false);
  assert.equal(r.issue?.code, "URL_SCHEME");
});

test("超长链接被拒绝", () => {
  const r = checkUrl(`https://example.com/${"a".repeat(2100)}`);
  assert.equal(r.ok, false);
  assert.equal(r.issue?.code, "URL_TOO_LONG");
});

test("非法链接被拒绝", () => {
  const r = checkUrl("https://");
  assert.equal(r.ok, false);
});

test("含空格的文本不会被误判为合法链接", () => {
  for (const raw of ["not a url at all!!", "abc def", "  中文 文本  "]) {
    const r = checkUrl(raw);
    assert.equal(r.ok, false, `${JSON.stringify(raw)} 应当被拒绝`);
    assert.ok(
      r.issue?.code === "URL_MALFORMED" || r.issue?.code === "URL_PRIVATE",
      `${JSON.stringify(raw)} 错误码应为 URL_MALFORMED，实际 ${r.issue?.code}`,
    );
  }
});

test("内网与回环地址被拦截", () => {
  const cases = [
    "https://localhost/a.mp4",
    "https://127.0.0.1/a.mp4",
    "https://192.168.1.10/a.mp4",
    "https://10.0.0.5/a.mp4",
    "https://172.16.3.4/a.mp4",
    "https://169.254.1.1/a.mp4",
    "https://nas.local/a.mp4",
  ];
  for (const url of cases) {
    const r = checkUrl(url);
    assert.equal(r.ok, false, `${url} 应当被拦截`);
    assert.equal(r.issue?.code, "URL_PRIVATE", `${url} 错误码应为 URL_PRIVATE`);
  }
});

test("公网地址通过", () => {
  assert.equal(isPrivateHost("www.bilibili.com"), false);
  assert.equal(isPrivateHost("8.8.8.8"), false);
  assert.equal(isPrivateHost("172.32.0.1"), false);
});