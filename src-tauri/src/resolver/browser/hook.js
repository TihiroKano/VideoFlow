/**
 * Browser Resolver 注入脚本。
 *
 * 抖音的详情接口要求 `a_bogus` 签名，纯 HTTP 客户端一律 403——实测即使补上真实
 * 浏览器登录态与 Uifid 头，错误也只是从 `Uifid Not Found` 前进到
 * `Signature Not Found`。所以我们不碰签名算法，只让页面自己按正常流程去请求，
 * 我们在旁边把结果捞回来。
 *
 * ## 三层提取（按可靠性从高到低，命中即用）
 *
 * 1. 站点结构化：抖音的 `aweme_detail`，能拿到清晰度阶梯与无水印直链
 * 2. DOM `<video>`：读 `currentSrc` / `src` 与 `<source>`（快手就是这条）
 * 3. 响应体扫描：匹配 `.m3u8` / `.mp4` 地址（推特等 HLS 站点的兜底）
 *
 * 两条铁律：
 * 1. 绝不改变页面行为——所有钩子都原样返回原始结果，只旁路读一份数据；
 * 2. 只回传必要字段与媒体地址，不回传完整响应（抖音一份就有 125 KB，
 *    且含大量不需要的用户/风控信息）。
 */
(function () {
  "use strict";

  // 可能被重复注入（多次导航），只装一次
  if (window.__VF_BROWSER_RESOLVER__) return;
  window.__VF_BROWSER_RESOLVER__ = true;

  var DETAIL_PATH = "aweme/v1/web/aweme/detail";
  var SENTINEL_HOST = "vf-capture.invalid";
  var SENTINEL_PARAM = "d";

  // 通用嗅探的防抖：页面可能陆续发多个请求，等安静下来再回传。
  // 给得比「最后一个候选」更宽裕，是为了等页面标题变成视频标题——
  // 太早回传会把通用站名当成文件名（实测快手会拿到「短视频-快手」）。
  var SNIFF_DEBOUNCE_MS = 3500;
  // 上限：候选太多时只留前面这些，避免把导航 URL 撑得过长
  var MAX_MEDIA = 8;

  var sent = false;
  var sniffTimer = null;
  var seen = {};
  /** 来自 <video> 元素的地址：正在播放的那条，最可信，排在前面 */
  var domUrls = [];
  /** 来自响应体扫描的地址：可能含广告或预加载片段，排在后面 */
  var sniffUrls = [];

  function log(msg) {
    try {
      console.log("[VideoFlow] " + msg);
    } catch (e) {
      /* 忽略 */
    }
  }

  /** gear_name 形如 normal_1080_0 / adapt_low_540_0，取其中的分辨率 */
  function heightFromGear(gear) {
    if (typeof gear !== "string") return null;
    var m = gear.match(/(\d{3,4})/);
    return m ? parseInt(m[1], 10) : null;
  }

  /**
   * 取一行清晰度阶梯里的分辨率，优先用 play_addr 的尺寸，退到 gear_name
   */
  function entryHeight(entry) {
    var pa = entry && entry.play_addr;
    if (pa) {
      var h = pa.height || pa.width;
      if (typeof h === "number" && h > 0) return h;
    }
    return heightFromGear(entry && entry.gear_name);
  }

  /**
   * 这一档的编码标识，形如 avc1 / hvc1 / hev1 / av01。
   *
   * 抖音的 bit_rate 数组里带 `is_bytevc1`（1 表示 H.265）与 `codec_type`，
   * 不同版本字段名不完全一致，所以几个来源都读一遍，读不到就返回 null。
   */
  function entryCodec(entry) {
    if (!entry || typeof entry !== "object") return null;
    var named = entry.codec_type || entry.codec || entry.video_codec;
    if (typeof named === "string" && named) {
      var lower = named.toLowerCase();
      // 站点的写法五花八门，统一成 ffmpeg 认的 codec tag
      if (lower.indexOf("bytevc1") !== -1 || lower.indexOf("hevc") !== -1 || lower.indexOf("h265") !== -1) {
        return "hvc1";
      }
      if (lower.indexOf("bytevc2") !== -1 || lower.indexOf("av1") !== -1) return "av01";
      if (lower.indexOf("avc") !== -1 || lower.indexOf("h264") !== -1) return "avc1";
      return lower;
    }
    if (entry.is_bytevc1 === 1 || entry.is_bytevc1 === true) return "hvc1";
    if (entry.is_bytevc1 === 0 || entry.is_bytevc1 === false) return "avc1";
    return null;
  }

  /**
   * 编码兼容性排序：数字越小越优先。
   *
   * 这条规则与 yt-dlp 那条路（`resolver/ytdlp.rs::video_codec_rank`）保持一致。
   * 为什么必须排：抖音 4K 档只给 H.265，而 Windows 自带播放器与不少播放器
   * 解不了 H.265——实测下载下来的 4K 文件本身完好（ffmpeg 能完整解码），
   * 但用户双击打不开，看起来就像「下载坏了」。
   */
  function codecRank(codec) {
    if (!codec) return 5;
    var c = codec.toLowerCase();
    if (c.indexOf("avc") === 0 || c.indexOf("h264") === 0) return 0;
    if (c.indexOf("hev") === 0 || c.indexOf("hvc") === 0 || c.indexOf("h265") === 0) return 1;
    if (c.indexOf("av01") === 0) return 2;
    if (c.indexOf("vp9") === 0 || c.indexOf("vp09") === 0) return 3;
    return 4;
  }

  function firstUrl(list) {
    if (!list || !list.length) return null;
    for (var i = 0; i < list.length; i++) {
      if (typeof list[i] === "string" && list[i]) return list[i];
    }
    return null;
  }

  /**
   * 把 bit_rate 数组压成「一档清晰度一条」。
   *
   * 实测一次响应会有 22 条、同一分辨率重复出现（含 h264/h265 多个版本），
   * 不去重会让清晰度下拉框塞满重复项。
   *
   * 同档的取舍顺序：**先比编码兼容性，再比码率**。此前只比码率，
   * 结果 4K 档选中的是码率更高但兼容性最差的 H.265（见 codecRank 的说明）。
   */
  function buildLadder(bitRates) {
    if (!bitRates || !bitRates.length) return [];
    var best = {};
    for (var i = 0; i < bitRates.length; i++) {
      var entry = bitRates[i];
      var height = entryHeight(entry);
      var url = firstUrl(entry && entry.play_addr && entry.play_addr.url_list);
      if (!height || !url) continue;
      var codec = entryCodec(entry);
      var rank = codecRank(codec);
      var rate = typeof entry.bit_rate === "number" ? entry.bit_rate : 0;
      var cand = { label: height + "p", bitRate: rate, urls: [url], codec: codec, rank: rank };
      var cur = best[height];
      if (!cur || rank < cur.rank || (rank === cur.rank && rate > cur.bitRate)) {
        best[height] = cand;
      }
    }
    var out = [];
    for (var k in best) {
      if (Object.prototype.hasOwnProperty.call(best, k)) {
        var picked = best[k];
        // rank 只是排序用的中间量，不下发给 Rust
        out.push({ label: picked.label, bitRate: picked.bitRate, urls: picked.urls, codec: picked.codec });
      }
    }
    return out;
  }

  /** 从抖音详情响应里摘出我们真正需要的字段 */
  function minimize(detail) {
    if (!detail || typeof detail !== "object") return null;
    var video = detail.video || {};
    var play = (video.play_addr && video.play_addr.url_list) || [];
    var cover =
      firstUrl((video.cover && video.cover.url_list) || null) ||
      firstUrl((video.origin_cover && video.origin_cover.url_list) || null);

    var payload = {
      source: "aweme",
      desc: typeof detail.desc === "string" ? detail.desc : null,
      pageTitle: document.title || null,
      durationMs: typeof video.duration === "number" ? video.duration : null,
      cover: cover,
      play: play.filter(function (u) {
        return typeof u === "string" && u;
      }),
      ladder: buildLadder(video.bit_rate),
    };

    if (!payload.play.length && !payload.ladder.length) return null;
    return payload;
  }

  /** UTF-8 安全的 base64url（无填充），避免中文标题在编码时损坏 */
  function encodePayload(obj) {
    var json = JSON.stringify(obj);
    var bytes = new TextEncoder().encode(json);
    var binary = "";
    for (var i = 0; i < bytes.length; i++) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }

  function send(payload) {
    if (sent) return;
    if (!payload) return;
    sent = true;
    try {
      var url =
        "https://" + SENTINEL_HOST + "/?" + SENTINEL_PARAM + "=" + encodePayload(payload);
      log("已抓到媒体数据（来源 " + (payload.source || "?") + "），回传给主进程");
      // 该导航会被主进程拦下并取消，不会产生真实请求
      window.location.href = url;
    } catch (e) {
      sent = false;
      log("回传失败：" + e);
    }
  }

  /** 第一层：结构化数据命中，立刻回传（信息最全） */
  function handleDetail(raw) {
    try {
      var payload = minimize(raw);
      if (payload) send(payload);
    } catch (e) {
      log("解析响应失败：" + e);
    }
  }

  // ---- 第二、三层：通用嗅探 ----

  /** 媒体地址是否值得收集（排除极小的图标/预览片段） */
  function isMediaUrl(url) {
    if (typeof url !== "string" || url.length < 12) return false;
    if (!/^https?:\/\//i.test(url)) return false;
    return /\.(m3u8|mp4|m4v|webm)(\?|$)/i.test(url);
  }

  /**
   * 收一个候选地址。
   *
   * `fromDom` 为真表示它来自页面上正在播放的 `<video>`——那是浏览器实际选中的
   * 那条流，可信度最高；为假表示来自响应体里的字面量（快手页面里就藏着好几条
   * 不同清晰度的地址），可能有广告或预加载片段。分开存是为了回传时能按
   * 可靠性排好序，让下载列表第一条就是对的（Rust 侧是无字段可比的稳定排序，
   * 不会打乱我们的输入顺序）。
   */
  function collect(url, fromDom) {
    if (sent || !isMediaUrl(url)) return;
    if (seen[url]) return;
    if (domUrls.length + sniffUrls.length >= MAX_MEDIA) return;
    seen[url] = true;
    if (fromDom) {
      domUrls.push(url);
    } else {
      sniffUrls.push(url);
    }

    // 收齐后等页面安静下来再回传，避免抓到播放器早期的低清占位流
    if (sniffTimer) clearTimeout(sniffTimer);
    sniffTimer = setTimeout(flushSniff, SNIFF_DEBOUNCE_MS);
  }

  /** 第二层：读页面上 <video> 元素当前的播放地址（快手走这条） */
  function collectFromDom() {
    try {
      var videos = document.querySelectorAll("video");
      for (var i = 0; i < videos.length; i++) {
        var v = videos[i];
        // currentSrc 是浏览器实际选中的地址，比 src 更可靠
        collect(v.currentSrc || v.src, true);
        var sources = v.querySelectorAll("source");
        for (var j = 0; j < sources.length; j++) {
          collect(sources[j].src, true);
        }
      }
    } catch (e) {
      /* 忽略 */
    }
  }

  /**
   * 标题取值顺序：og:title → twitter:title → document.title。
   *
   * 前两个是页面作者为「这一条内容」写的标题，`document.title` 在快手这类站点上
   * 会先渲染成通用站名（实测拿到「短视频-快手」），所以放在最后。
   */
  function bestTitle() {
    function meta(selector) {
      try {
        var el = document.querySelector(selector);
        var text = el && el.getAttribute("content");
        return typeof text === "string" && text.trim() ? text.trim() : null;
      } catch (e) {
        return null;
      }
    }
    return (
      meta('meta[property="og:title"]') ||
      meta('meta[name="twitter:title"]') ||
      (document.title || "").trim() ||
      null
    );
  }

  function flushSniff() {
    if (sent) return;
    collectFromDom();
    if (!domUrls.length && !sniffUrls.length) return;
    send({
      source: "sniff",
      desc: null,
      pageTitle: bestTitle(),
      // 分开发：<video> 里正在播的那条最可信；响应体扫描出来的可能是推荐流的
      // 下一条视频（实测快手页面里就混着另一条），只有 DOM 拿不到时才用它。
      dom: domUrls,
      sniff: sniffUrls,
    });
  }

  /** 第三层：从响应体文本里扫媒体地址 */
  function scanBody(text) {
    if (sent || typeof text !== "string" || text.length < 64) return;
    // 只在看起来像媒体描述的内容里扫，避免把整页 HTML 全扫一遍
    if (text.indexOf(".m3u8") === -1 && text.indexOf(".mp4") === -1) return;
    var re = /https?:\/\/[^"'\\\s]{10,400}?\.(?:m3u8|mp4|m4v)[^"'\\\s]*/gi;
    var m;
    while ((m = re.exec(text)) !== null) {
      collect(m[0], false);
    }
  }

  // ---- 钩子安装 ----

  var origFetch = window.fetch;
  if (typeof origFetch === "function") {
    window.fetch = function () {
      var args = arguments;
      var promise = origFetch.apply(this, args);
      try {
        var input = args[0];
        var url = typeof input === "string" ? input : input && input.url;
        if (typeof url === "string") {
          if (isMediaUrl(url)) collect(url, false);
          // 用 clone 读一份，绝不动原始响应体
          if (url.indexOf(DETAIL_PATH) !== -1 || /graphql|\/api\//i.test(url)) {
            promise
              .then(function (resp) {
                resp
                  .clone()
                  .text()
                  .then(function (text) {
                    if (text.indexOf("aweme_detail") !== -1) {
                      try {
                        var json = JSON.parse(text);
                        if (json && json.aweme_detail) handleDetail(json.aweme_detail);
                      } catch (e) {
                        /* 忽略 */
                      }
                    }
                    scanBody(text);
                  })
                  .catch(function () {});
              })
              .catch(function () {});
          }
        }
      } catch (e) {
        /* 旁路失败绝不能影响页面 */
      }
      return promise;
    };
  }

  var origOpen = XMLHttpRequest.prototype.open;
  var origSend = XMLHttpRequest.prototype.send;

  XMLHttpRequest.prototype.open = function (method, url) {
    try {
      this.__vf_url = typeof url === "string" ? url : "";
    } catch (e) {
      /* 忽略 */
    }
    return origOpen.apply(this, arguments);
  };

  XMLHttpRequest.prototype.send = function () {
    try {
      var self = this;
      var url = self.__vf_url || "";
      if (isMediaUrl(url)) collect(url, false);
      if (url.indexOf(DETAIL_PATH) !== -1 || /graphql|\/api\//i.test(url)) {
        self.addEventListener("load", function () {
          try {
            var text =
              self.responseType === "" || self.responseType === "text"
                ? self.responseText
                : self.responseType === "json"
                  ? JSON.stringify(self.response)
                  : null;
            if (!text) return;
            if (text.indexOf("aweme_detail") !== -1) {
              try {
                var json = JSON.parse(text);
                if (json && json.aweme_detail) handleDetail(json.aweme_detail);
              } catch (e) {
                /* 忽略 */
              }
            }
            scanBody(text);
          } catch (e) {
            /* 忽略 */
          }
        });
      }
    } catch (e) {
      /* 忽略 */
    }
    return origSend.apply(this, arguments);
  };

  // 播放器常常是先把 <video> 插进 DOM、随后才设 src；跟着 DOM 变化抓
  try {
    var mo = new MutationObserver(function () {
      collectFromDom();
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
  } catch (e) {
    /* 忽略 */
  }
  // 视频就绪事件也补一次
  document.addEventListener(
    "loadedmetadata",
    function () {
      collectFromDom();
    },
    true,
  );
  // 兜底：页面起来后再看一次 DOM（有些站点不做 DOM 变更）
  setTimeout(collectFromDom, 3000);
  setTimeout(collectFromDom, 6000);

  log("钩子已就绪（三层提取）");
})();