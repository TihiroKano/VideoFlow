/**
 * 液态玻璃着色器（WebGL2 / GLSL ES 3.00）
 *
 * 光学模型（见 docs/VideoFlow-项目书.md §5.1.1）：
 *   1. 圆角矩形 SDF 轮廓（fwidth 抗锯齿）
 *   2. 倒角高度场：中心平坦、仅倒角带连续弯曲（quintic）
 *   3. 双界面折射：Air → Glass → Air，追迹玻璃内光程后与背景平面求交
 *   4. 三路采样：Face（中心稳定）/ Inner Rim（边缘压缩）/ Outer Rim（外扩柔化）
 *   5. 光照顺序：REFRACT → SHADOW → TINT → LIGHT（GGX + Smith + Schlick）
 *
 * 质量档（L1）开启色散与更高抽样；平衡档（L2）关闭色散。
 */

/** 公共函数库 */
const COMMON = /* glsl */ `
const float PI = 3.14159265359;

/** 圆角矩形有符号距离，p 为相对中心坐标 */
float sdRoundBox(vec2 p, vec2 b, float r) {
  vec2 q = abs(p) - b + r;
  return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

/** quintic 平滑：一阶、二阶导在两端均为 0，保证倒角连续弯曲 */
float quintic(float t) {
  return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

float quinticD(float t) {
  return 30.0 * t * t * (t - 1.0) * (t - 1.0);
}

/** 背景渲染缩放：cover(0) / contain(1) */
float bgScale() {
  float sCover = max(uResolution.x / uImgSize.x, uResolution.y / uImgSize.y);
  float sContain = min(uResolution.x / uImgSize.x, uResolution.y / uImgSize.y);
  return mix(sCover, sContain, uFit);
}

/** 屏幕像素坐标 → 背景纹理 uv */
vec2 screenToBgUv(vec2 fragPx) {
  vec2 dispSize = uImgSize * bgScale();
  vec2 off = (dispSize - uResolution) * 0.5;
  return (fragPx + off) / dispSize;
}

/** 屏幕像素长度 → 背景 uv 长度 */
vec2 pxToBgUv(float px) {
  return vec2(px / (uImgSize.x * bgScale()), px / (uImgSize.y * bgScale()));
}
`;

export const GLASS_VERT = /* glsl */ `#version 300 es
in vec2 aPos;
out vec2 vUv;

void main() {
  vUv = aPos * 0.5 + 0.5;
  gl_Position = vec4(aPos, 0.0, 1.0);
}
`;

/** 背景直通绘制：把用户图片按 cover / contain 铺满画布 */
export const BG_FRAG = /* glsl */ `#version 300 es
precision highp float;

in vec2 vUv;
out vec4 fragColor;

uniform sampler2D uBg;
uniform vec2 uResolution;
uniform vec2 uImgSize;
uniform float uFit;
uniform vec3 uFallback;
uniform float uBgOpacity;

void main() {
  // vUv 的原点在左下（NDC y=+1 是屏幕顶部），这里翻成「左上原点」，
  // 与玻璃着色器里的 luv 保持同一坐标系；否则背景会被上下翻转，
  // 并在玻璃面板边缘出现明显错位接缝。
  vec2 luv = vec2(vUv.x, 1.0 - vUv.y);

  float sCover = max(uResolution.x / uImgSize.x, uResolution.y / uImgSize.y);
  float sContain = min(uResolution.x / uImgSize.x, uResolution.y / uImgSize.y);
  float scale = mix(sCover, sContain, uFit);
  vec2 dispSize = uImgSize * scale;
  vec2 off = (dispSize - uResolution) * 0.5;
  vec2 uv = (luv * uResolution + off) / dispSize;

  vec3 base = uFallback;
  // 极窄 / 极宽图片在 contain 下会露出画布，用深色兜底而不是透明
  if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
    fragColor = vec4(base, 1.0);
    return;
  }
  vec3 col = texture(uBg, uv).rgb;
  // 背景透明度：向底色过渡，保持与 CSS 档位的观感一致
  col = mix(base, col, clamp(uBgOpacity, 0.0, 1.0));
  fragColor = vec4(col, 1.0);
}
`;

/** 面板玻璃绘制 */
export const GLASS_FRAG = /* glsl */ `#version 300 es
precision highp float;

in vec2 vUv;
out vec4 fragColor;

uniform sampler2D uBg;
uniform vec2  uResolution;    // 画布尺寸（CSS px）
uniform vec4  uRect;          // 面板矩形 x, y, w, h（CSS px）
uniform vec2  uImgSize;
uniform float uFit;
uniform float uRadius;
uniform float uBevel;
uniform float uThickness;
uniform float uIor;
uniform float uRefract;
uniform float uBlurLod;
uniform float uTint;
uniform float uBrightness;
uniform float uOpacity;
uniform float uDispersion;
uniform vec3  uLightDir;
uniform float uBgOpacity;
uniform vec3  uBgBase;

${COMMON}

/** 在给定 mip 上做 5 抽样平滑，抑制单点采样的方块感 */
vec3 sampleBgSoft(vec2 uv, float lod, float radiusPx, vec2 dirA, vec2 dirB) {
  vec2 texel = pxToBgUv(radiusPx);
  vec3 acc = textureLod(uBg, uv, lod).rgb * 0.4;
  acc += textureLod(uBg, uv + dirA * texel, lod).rgb * 0.15;
  acc += textureLod(uBg, uv - dirA * texel, lod).rgb * 0.15;
  acc += textureLod(uBg, uv + dirB * texel, lod).rgb * 0.15;
  acc += textureLod(uBg, uv - dirB * texel, lod).rgb * 0.15;
  return acc;
}

void main() {
  // WebGL 的 vUv 原点在左下；翻转为「左上原点」的局部 uv，与 DOM 测量一致
  vec2 luv = vec2(vUv.x, 1.0 - vUv.y);

  // luv 是「整块画布」的归一化坐标，而一个面板只占画布中的一小块。
  // 必须先用 uRect 换算成面板局部坐标再算 SDF：否则圆角矩形会被摆在
  // 以画布中心为原点的位置，落在面板范围内的只是它的一个窗口，
  // 表现就是同一条边的两个角「一头圆、一头方」——离画布中心越远
  // 越靠外，越容易被裁成直角。
  vec2 sizePx = uRect.zw;
  vec2 canvasPx = luv * uResolution;
  vec2 p = canvasPx - uRect.xy - sizePx * 0.5;
  vec2 halfSize = sizePx * 0.5;

  // ---- 1. 轮廓 ----
  float d = sdRoundBox(p, halfSize, uRadius);
  float aa = max(fwidth(d), 0.0001);
  float mask = 1.0 - smoothstep(-aa, aa, d);
  if (mask <= 0.002) discard;

  // ---- 2. 高度场与法线（只对平滑高度场求导） ----
  float bevel = max(uBevel, 1.0);
  float t = clamp(-d / bevel, 0.0, 1.0);
  float h = quintic(t);

  vec2 g = vec2(
    sdRoundBox(p + vec2(1.0, 0.0), halfSize, uRadius) - sdRoundBox(p - vec2(1.0, 0.0), halfSize, uRadius),
    sdRoundBox(p + vec2(0.0, 1.0), halfSize, uRadius) - sdRoundBox(p - vec2(0.0, 1.0), halfSize, uRadius)
  );
  g = normalize(g + vec2(1e-6));

  float dh = quinticD(t) * (-1.0 / bevel);
  vec2 gradH = g * dh;
  vec3 n = normalize(vec3(-gradH * uThickness * 0.5, 1.0));

  // ---- 3. 双界面折射 ----
  vec3 I = vec3(0.0, 0.0, -1.0);
  vec3 R = refract(I, n, 1.0 / uIor);
  if (dot(R, R) < 1e-6) R = I;
  float glassThickness = uThickness * (1.0 - h * 0.45);
  vec3 P = R * (glassThickness / max(0.15, -R.z));
  vec2 refrUv = pxToBgUv(1.0) * vec2(P.x, P.y) * uRefract;

  // ---- 4. 三路采样 ----
  // fragPx 直接用画布坐标：换算成背景 uv 时才不会把背景采错位置
  vec2 fragPx = canvasPx;
  vec2 bgUv = screenToBgUv(fragPx);

  float rim = 1.0 - t;                       // 1 在边缘，0 在中心
  float blurPx = exp2(uBlurLod);

  vec2 dirA = normalize(vec2(0.8, 0.6));
  vec2 dirB = normalize(vec2(-0.6, 0.8));

  // 三路采样的额外抽样半径都很小：主要模糊来自 mip 层级，
  // 抽样只用于抹掉 mip 的方块感，避免两者叠加导致过度模糊。
  vec3 colFace = sampleBgSoft(bgUv, uBlurLod, blurPx * 0.18, dirA, dirB);
  vec2 innerUv = bgUv + refrUv;
  vec3 colInner = sampleBgSoft(innerUv, max(uBlurLod - 1.2, 0.0), blurPx * 0.1, dirA, dirB);
  vec2 outerUv = bgUv + g * pxToBgUv(blurPx * 0.3) + refrUv * 1.6;
  vec3 colOuter = sampleBgSoft(outerUv, min(uBlurLod + 0.8, 8.0), blurPx * 0.3, dirA, dirB);

#ifdef GLASS_QUALITY
  // 质量档：额外抽样压噪
  colInner = mix(colInner, sampleBgSoft(innerUv + dirA * pxToBgUv(blurPx * 0.18), max(uBlurLod - 1.2, 0.0), blurPx * 0.1, dirA, dirB), 0.35);
  colFace = mix(colFace, sampleBgSoft(bgUv + dirB * pxToBgUv(blurPx * 0.15), uBlurLod, blurPx * 0.18, dirA, dirB), 0.3);
#endif

  float wInner = smoothstep(0.0, 0.45, rim);
  float wOuter = smoothstep(0.55, 1.0, rim) * 0.85;
  float wFace = max(0.0, 1.0 - wInner - wOuter);
  vec3 col = colFace * wFace + colInner * wInner + colOuter * wOuter;

  // 背景透明度同样作用于玻璃采到的背景，避免调低透明度后玻璃比背景更亮；
  // 在着色与光照之前混合，因此玻璃自身的 tint 与光边不会被冲淡。
  col = mix(uBgBase, col, clamp(uBgOpacity, 0.0, 1.0));

  // 与 CSS 档的 --vf-glass-fill 对齐：那一档在背景之上还叠了一层半透明白，
  // 这里补上同样的「抬底」。否则 Liquid Glass 档的底比 CSS 档暗约 20 级，
  // 叠在上面的细白描边（虚线投放框、按钮描边、控件描边）会变成刺眼的亮线
  // —— 用户反馈的「切换到 Liquid Glass 后出现横向/纵向亮线」就是这个。
  // 只影响明度，不动任何几何：布局、尺寸、间距、圆角与位置全部不变。
  col = mix(col, vec3(1.0), 0.12);

  // ---- 5. 色散：仅质量档，且只作用于边缘，中心禁止明显色散 ----
#ifdef GLASS_QUALITY
  float dispAmt = uDispersion * smoothstep(0.5, 1.0, rim);
  if (dispAmt > 0.0005) {
    vec2 k = pxToBgUv(dispAmt) * 6.0;
    float rCh = textureLod(uBg, innerUv + k, max(uBlurLod - 1.2, 0.0)).r;
    float bCh = textureLod(uBg, innerUv - k, max(uBlurLod - 1.2, 0.0)).b;
    col = mix(col, vec3(rCh, col.g, bCh), smoothstep(0.55, 1.0, rim) * 0.5);
  }
#endif

  // ---- 6. 阴影：贴边内侧轻微压暗，制造厚度 ----
  // 幅度保持很小，否则整块玻璃会发灰、失去预览图里通透明亮的感觉
  float innerShadow = smoothstep(0.0, 0.28, rim) * (1.0 - smoothstep(0.28, 0.62, rim));
  col *= 1.0 - innerShadow * 0.06;

  // ---- 7. 着色：偏冷的乳白 ----
  vec3 tintCol = vec3(0.94, 0.97, 1.0);
  // 以乘法为主，保留背景的蓝色饱和度，避免整块玻璃发灰
  col *= mix(vec3(1.0), tintCol, uTint);
  // 乳白薄雾：抬升暗部，模拟玻璃的漫透射，让面板更通透发亮
  col = mix(col, vec3(0.80, 0.88, 0.98), uTint * 0.24);
  col *= uBrightness;

  // ---- 8. 光照：GGX + Smith + Schlick，限制在倒角外半段 ----
  vec3 V = vec3(0.0, 0.0, 1.0);
  vec3 L = normalize(uLightDir);
  vec3 Hv = normalize(L + V);
  float NoH = max(dot(n, Hv), 0.0);
  float NoV = max(dot(n, V), 1e-4);
  float NoL = max(dot(n, L), 0.0);

  float rough = 0.22;
  float a = rough * rough;
  float a2 = a * a;
  float dGGX = a2 / (PI * pow(NoH * NoH * (a2 - 1.0) + 1.0, 2.0));
  float k = a * 0.5;
  float gSmith = (NoL / (NoL * (1.0 - k) + k)) * (NoV / (NoV * (1.0 - k) + k));
  float f0 = 0.045;
  float fres = f0 + (1.0 - f0) * pow(1.0 - max(dot(Hv, V), 0.0), 5.0);
  float spec = dGGX * gSmith * fres;

  float bevelBand = smoothstep(0.0, 0.55, rim);
  col += vec3(1.0, 0.98, 0.95) * spec * bevelBand * 2.4;

  float rimFres = pow(1.0 - NoV, 3.0);
  col += vec3(0.85, 0.93, 1.0) * rimFres * bevelBand * 0.5;

  // 玻璃的「透」来自模糊与折射后的背景，而不是降低不透明度：
  // 用较高的 alpha 让模糊结果主导，避免清晰背景透上来削弱玻璃质感。
  float alpha = clamp(uOpacity + rimFres * 0.12, 0.0, 1.0) * mask;
  fragColor = vec4(col, alpha);
}
`;