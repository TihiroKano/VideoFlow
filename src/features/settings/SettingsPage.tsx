/**
 * 设置页（项目书 §2.2 设置页）。
 * 下载、外观、渲染状态与诊断分组；切换档位不重启应用、不中断下载。
 */

import { useCallback, useEffect, useState } from "react";
import { GlassSelect } from "@/components/controls/GlassSelect";
import { GlassSurface } from "@/components/glass/GlassSurface";
import { IconFolder } from "@/components/icons";
import {
  accountConfirmLogin,
  accountLogin,
  accountLogout,
  accountStatus,
  cookieSourceStatus,
  describeError,
  pickDirectory,
  sidecarStatus,
  type AccountSiteStatus,
  type CookieSourceStatus,
  type NetworkPathProbe,
} from "@/services/ipc";
import { profileDevice, type GlassTier } from "@/services/deviceProfile";
import {
  PROXY_MODES,
  PROXY_TYPES,
  validateProxyHost,
  type ProxyMode,
  type ProxyType,
} from "@/features/settings/proxyConfig";
import { useSettingsStore, type TierChoice } from "@/stores/settingsStore";
import { useUiStore } from "@/stores/uiStore";

const TIER_OPTIONS: { value: TierChoice; label: string; hint: string }[] = [
  { value: "auto", label: "自动", hint: "按本机能力推荐" },
  { value: "quality", label: "质量 L1", hint: "完整折射与色散" },
  { value: "balanced", label: "平衡 L2", hint: "保留折射，关闭色散" },
  { value: "performance", label: "性能 L3", hint: "纯 CSS，不初始化 WebGL" },
];

const TIER_CODE: Record<GlassTier, string> = {
  quality: "L1",
  balanced: "L2",
  performance: "L3",
};

/**
 * 登录态来源。
 *
 * 抖音没有登录态就拿不到播放地址；Bilibili 的 1080P 60 帧、1080P+ 等清晰度也只对
 * 已登录账号开放。这里用的是用户自己浏览器里本来就有的会话，属于使用本人账号
 * 已有的权限，不涉及绕过任何访问控制。
 *
 * 两条通路的取舍：Chromium 系浏览器运行时会锁住 Cookie 数据库，yt-dlp 复制不出来
 * （上游 issue #7271），此时必须完全退出浏览器；cookies.txt 文件不受此影响。
 */
const COOKIE_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "不使用" },
  { value: "browser:edge", label: "Microsoft Edge" },
  { value: "browser:chrome", label: "Google Chrome" },
  { value: "browser:firefox", label: "Firefox" },
  { value: "file:", label: "cookies.txt 文件…" },
];

type CookieTone = "ok" | "warn" | "muted";

/** 一条出口探测结果的显示文本（没结果时给「—」，不让界面出现空白格） */
function probeText(probe: NetworkPathProbe | null | undefined): string {
  if (!probe) return "—";
  const latency = probe.latencyMs ? ` · ${probe.latencyMs} ms` : "";
  return `${probe.available ? "可用" : "不可用"}：${probe.detail}${latency}`;
}

/**
 * 把诊断结果翻译成一句「现在能不能用」。
 *
 * 重点是浏览器开着这种情况：它是抖音解析失败最常见的原因，却最难自查，
 * 所以文案要直接点名浏览器，并给出「改用 cookies.txt」这条不用关浏览器的出路。
 */
function describeCookieStatus(
  status: CookieSourceStatus | null,
  checking: boolean,
): { tone: CookieTone; text: string } {
  if (checking || !status) return { tone: "muted", text: "检测中…" };

  switch (status.kind) {
    case "none":
      return { tone: "muted", text: "未启用。抖音必须开启，Bilibili 的高清晰度也需要它" };
    case "unknown":
      return { tone: "warn", text: "来源无法识别，请重新选择" };
    case "file":
      return status.fileExists
        ? { tone: "ok", text: "cookies.txt 已就绪，浏览器开着也能用" }
        : { tone: "warn", text: "找不到这个 cookies.txt，文件可能被移动或删除，请重新选择" };
    case "account": {
      const sites = status.accountSites ?? [];
      if (!status.accountFileExists) {
        return { tone: "warn", text: "还没有登录任何站点，请在上面点「登录」" };
      }
      if (sites.length === 0) {
        return { tone: "warn", text: "已有 Cookie 文件但没有识别到登录态，请重新登录" };
      }
      return { tone: "ok", text: `已登录：${sites.join("、")}，解析时会带上这份登录态` };
    }
    case "browser": {
      const name = status.browserLabel ?? "浏览器";
      if (status.browserRunning === true) {
        return {
          tone: "warn",
          text: `检测到 ${name} 正在运行，它锁住了 Cookie 数据库，解析前需要完全退出（含后台进程）；不想关浏览器就改用「cookies.txt 文件」`,
        };
      }
      if (status.browserRunning === false) {
        return { tone: "ok", text: `${name} 未在运行，可以读取它的登录状态` };
      }
      return { tone: "muted", text: `无法确认 ${name} 是否正在运行` };
    }
  }
}

export function SettingsPage(): React.JSX.Element {
  const s = useSettingsStore();
  const pushToast = useUiStore((t) => t.pushToast);
  const [sidecarError, setSidecarError] = useState<string | null>(null);
  const [cookieStatus, setCookieStatus] = useState<CookieSourceStatus | null>(null);
  const [checkingCookies, setCheckingCookies] = useState(false);
  const [sites, setSites] = useState<AccountSiteStatus[] | null>(null);
  /** 正在登录的站点 key；非空时禁用该行按钮 */
  const [loggingIn, setLoggingIn] = useState<string | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  /** 代理地址的输入草稿：只在失焦/回车时提交，避免每敲一个字就打一次 IPC */
  const [proxyHostDraft, setProxyHostDraft] = useState(s.proxyHost);

  useEffect(() => {
    setProxyHostDraft(s.proxyHost);
  }, [s.proxyHost]);

  const proxyHostError = proxyHostDraft.trim() ? validateProxyHost(proxyHostDraft) : null;

  const commitProxyHost = useCallback(() => {
    const value = proxyHostDraft.trim();
    if (value && value !== s.proxyHost) s.setProxyHost(value);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [proxyHostDraft, s.proxyHost]);

  // 诊断结论的色调：可达=ok；当前出口有问题=warn；其余（未检测/直连不通但不算配置问题）=muted
  const networkTone: CookieTone = !s.network
    ? "muted"
    : s.network.xReachable
      ? "ok"
      : s.network.proxyOk
        ? "muted"
        : "warn";

  useEffect(() => {
    if (s.profile || s.profiling) return;
    s.setProfiling(true);
    profileDevice()
      .then((p) => s.setProfile(p))
      .catch(() => undefined)
      .finally(() => s.setProfiling(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // sidecar 探测会把 yt-dlp 与 ffmpeg 真的拉起来取版本（本机约 2–3 秒），
  // 因此一次会话只做一次：结果存在 store 里，再次进入设置页直接复用。
  useEffect(() => {
    if (s.sidecar) return;
    sidecarStatus()
      .then((status) => s.setSidecar(status))
      .catch((err) => setSidecarError(err instanceof Error ? err.message : "无法读取"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * 探测登录态来源是否可用。
   *
   * 每次切换来源都重测：浏览器是开是关随时会变，缓存下来反而会误导用户。
   * 后台未就绪（纯浏览器预览）时静默留空，界面按「检测中」展示。
   */
  const detectCookies = useCallback(async () => {
    setCheckingCookies(true);
    try {
      setCookieStatus(await cookieSourceStatus(s.cookiesSource));
    } catch {
      setCookieStatus(null);
    } finally {
      setCheckingCookies(false);
    }
  }, [s.cookiesSource]);

  useEffect(() => {
    void detectCookies();
  }, [detectCookies]);

  /** 读账户登录状态；纯浏览器预览下静默留空 */
  const refreshSites = useCallback(async () => {
    try {
      setSites(await accountStatus());
    } catch {
      setSites(null);
    }
  }, []);

  useEffect(() => {
    void refreshSites();
  }, [refreshSites]);

  /**
   * 登录：会一直等到用户扫码完成（或点「我已完成登录」），期间按钮显示进行中。
   * 抖音/快手无法自动判定登录成功，需要用户点那个按钮收工，所以这里提示要说清。
   */
  const handleLogin = async (site: AccountSiteStatus) => {
    setLoggingIn(site.key);
    try {
      const next = await accountLogin(site.key);
      setSites(next);
      void detectCookies();
      const me = next.find((x) => x.key === site.key);
      const detail = me?.userName
        ? `已登录：${me.userName}${me.isVip ? "（大会员）" : ""}`
        : undefined;
      pushToast({
        level: "success",
        message: `${site.label} 登录成功`,
        detail,
        persistent: false,
      });
    } catch (err) {
      pushToast({
        level: "warning",
        message: `${site.label} 登录未完成`,
        detail: describeError(err),
        // 登录是低风险可重试操作（尤其「已取消登录」只是用户主动关窗），
        // 不必像文件损坏那样持久贴在屏幕上要人复盘，几秒后自动消失即可。
        persistent: false,
      });
    } finally {
      setLoggingIn(null);
    }
  };

  /** 抖音/快手的登录 Cookie 名不稳定，只能由用户确认登录已完成 */
  const handleConfirm = async (siteKey: string) => {
    try {
      await accountConfirmLogin(siteKey);
    } catch {
      // 确认失败不影响用户改点「已完成」重试
    }
  };

  const handleLogout = async (site: AccountSiteStatus) => {
    try {
      const next = await accountLogout(site.key);
      setSites(next);
      void detectCookies();
      pushToast({ level: "success", message: `已退出 ${site.label}`, persistent: false });
    } catch (err) {
      pushToast({
        level: "warning",
        message: `退出 ${site.label} 失败`,
        detail: describeError(err),
        persistent: true,
      });
    }
  };

  const chooseDir = async () => {
    try {
      const dir = await pickDirectory(s.downloadDir);
      if (dir) {
        s.setDownloadDir(dir);
        pushToast({ level: "success", message: `下载目录已改为 ${dir}`, persistent: false });
      }
    } catch {
      pushToast({
        level: "warning",
        message: "无法打开目录选择器",
        detail: "请在 VideoFlow 应用窗口中使用此功能。",
        persistent: true,
      });
    }
  };

  /** cookies.txt 是 Netscape 格式的纯文本，用扩展从浏览器导出后在这里选择 */
  const chooseCookiesFile = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "Cookie 文件", extensions: ["txt"] }],
      });
      if (typeof picked !== "string") {
        // 用户取消：回到「不使用」，避免停在一个没有文件的选择上
        if (s.cookiesSource === "file:") s.setCookiesSource("");
        return;
      }
      s.setCookiesSource(`file:${picked}`);
      pushToast({ level: "success", message: "已设置 Cookie 文件", detail: picked, persistent: false });
    } catch (err) {
      pushToast({
        level: "warning",
        message: "无法选择 Cookie 文件",
        detail: err instanceof Error ? err.message : undefined,
        persistent: true,
      });
    }
  };

  const cookiesFile = s.cookiesSource.startsWith("file:") ? s.cookiesSource.slice(5) : "";
  // 具体文件路径不对应任何一个 option，下拉框统一显示成「cookies.txt 文件…」
  const cookiesChoice = s.cookiesSource.startsWith("file:") ? "file:" : s.cookiesSource;
  const cookieDesc = describeCookieStatus(cookieStatus, checkingCookies);

  return (
    <GlassSurface variant="panel" className="vf-settings" id="glass-settings" tint={0.5} opacity={0.88}>
      <header className="vf-settings__header">
        <h1 className="vf-settings__title">设置</h1>
      </header>

      <div className="vf-settings__body vf-scroll">
        <section className="vf-settings__group" aria-labelledby="vf-set-download">
          <h2 id="vf-set-download" className="vf-settings__group-title">
            下载
          </h2>

          <div className="vf-settings__row">
            <div className="vf-settings__row-label">
              <span>保存位置</span>
              <span className="vf-settings__row-hint">默认放在 D 盘，避免占用系统盘空间</span>
            </div>
            <div className="vf-settings__row-control">
              <span className="vf-settings__path vf-truncate" title={s.downloadDir}>
                {s.downloadDir}
              </span>
              <button type="button" className="vf-btn-ghost vf-settings__pick" onClick={() => void chooseDir()}>
                <IconFolder size={17} />
                <span>更改</span>
              </button>
            </div>
          </div>

          <div className="vf-settings__row">
            <div className="vf-settings__row-label">
              <span>同时下载任务数</span>
              <span className="vf-settings__row-hint">1 – 6，默认 3</span>
            </div>
            <div className="vf-settings__row-control">
              <input
                type="range"
                className="vf-slider__input vf-slider__input--inline"
                min={1}
                max={6}
                step={1}
                value={s.maxConcurrent}
                aria-label="同时下载任务数"
                onChange={(e) => s.setMaxConcurrent(Number(e.target.value))}
              />
              <span className="vf-settings__value">{s.maxConcurrent}</span>
            </div>
          </div>

          <div className="vf-settings__row">
            <div className="vf-settings__row-label">
              <span>单任务分段并发</span>
              <span className="vf-settings__row-hint">1 – 8，仅对支持 Range 的直链生效</span>
            </div>
            <div className="vf-settings__row-control">
              <input
                type="range"
                className="vf-slider__input vf-slider__input--inline"
                min={1}
                max={8}
                step={1}
                value={s.chunkConcurrency}
                aria-label="单任务分段并发"
                onChange={(e) => s.setChunkConcurrency(Number(e.target.value))}
              />
              <span className="vf-settings__value">{s.chunkConcurrency}</span>
            </div>
          </div>

          <div className="vf-settings__row">
            <div className="vf-settings__row-label">
              <span>同一来源并发上限</span>
              <span className="vf-settings__row-hint">
                同一个站点同时下载的任务数，1 – 6；避免开太多连接触发服务端限流
              </span>
            </div>
            <div className="vf-settings__row-control">
              <input
                type="range"
                className="vf-slider__input vf-slider__input--inline"
                min={1}
                max={6}
                step={1}
                value={s.perHostConcurrency}
                aria-label="同一来源并发上限"
                onChange={(e) => s.setPerHostConcurrency(Number(e.target.value))}
              />
              <span className="vf-settings__value">{s.perHostConcurrency}</span>
            </div>
          </div>

          <p className="vf-settings__note">
            设备档位偏低（{TIER_CODE[s.effectiveTier]}）时，主进程会自动下调上面两个并发值，
            避免弱机同时开太多连接拖慢下载与界面。
          </p>

          <div className="vf-settings__row vf-settings__row--stack">
            <div className="vf-settings__row-label">
              <span>账户登录</span>
              <span className="vf-settings__row-hint">
                登录后解析会带上你的账号权限。Bilibili 的 1080P 60 帧需要登录，
                4K 与 1080P 高码率需要大会员；抖音、快手建议登录。只在本机保存，不会上传
              </span>
            </div>
            <div className="vf-settings__accounts">
              {(sites ?? []).map((site) => (
                <div key={site.key} className="vf-settings__account">
                  <span className="vf-settings__account-name">{site.label}</span>
                  <span
                    className={`vf-settings__account-state vf-settings__cookie-desc--${
                      site.loggedIn ? "ok" : "muted"
                    }`}
                  >
                    {site.loggedIn
                      ? site.userName
                        ? `${site.userName}${site.isVip ? " · 大会员" : ""}`
                        : "已登录"
                      : "未登录"}
                  </span>
                  <span className="vf-settings__account-actions">
                    {site.loggedIn ? (
                      <button
                        type="button"
                        className="vf-btn-ghost vf-settings__pick"
                        onClick={() => void handleLogout(site)}
                      >
                        退出
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="vf-btn-ghost vf-settings__pick"
                        disabled={loggingIn !== null}
                        onClick={() => void handleLogin(site)}
                      >
                        {loggingIn === site.key ? "等待登录…" : "登录"}
                      </button>
                    )}
                    {/* 登录进行中且该站点无法自动判定时，给用户一个收工的入口 */}
                    {loggingIn === site.key && site.needsManualConfirm ? (
                      <button
                        type="button"
                        className="vf-btn-ghost vf-settings__pick"
                        onClick={() => void handleConfirm(site.key)}
                      >
                        我已完成登录
                      </button>
                    ) : null}
                  </span>
                </div>
              ))}
              {sites === null ? (
                <p className="vf-settings__row-hint">账户状态读取中…</p>
              ) : null}
            </div>
            <p className="vf-settings__note">
              4K 需要 Bilibili 大会员，这是账号权限而非程序限制；登录后会在这里显示是否为大会员。
            </p>
          </div>

          <div className="vf-settings__row vf-settings__row--stack">
            <div className="vf-settings__row-label">
              <span>登录状态检查</span>
              <span className="vf-settings__row-hint">解析前会在这里直接告诉你现在能不能用</span>
            </div>
            <div className="vf-settings__row-control">
              <span className={`vf-settings__cookie-desc vf-settings__cookie-desc--${cookieDesc.tone}`}>
                {cookieDesc.text}
              </span>
              <button
                type="button"
                className="vf-btn-ghost vf-settings__pick"
                onClick={() => void detectCookies()}
                disabled={checkingCookies}
              >
                {checkingCookies ? "检测中…" : "重新检测"}
              </button>
            </div>
          </div>

          {/* 其他方式：保留给已用浏览器登录、或习惯自己导出 cookies.txt 的用户 */}
          <div className="vf-settings__row vf-settings__row--stack">
            <button
              type="button"
              className="vf-settings__disclosure"
              aria-expanded={showAdvanced}
              onClick={() => setShowAdvanced((v) => !v)}
            >
              {showAdvanced ? "▾" : "▸"} 其他方式（浏览器登录态 / cookies.txt 文件）
            </button>
            {showAdvanced ? (
              <div className="vf-settings__advanced">
                <div className="vf-settings__row-label">
                  <span className="vf-settings__row-hint">
                    想直接复用浏览器里已有的登录态就选浏览器，但浏览器运行时它锁住了 Cookie
                    数据库，需要完全退出；也可以自己用扩展导出 cookies.txt 后在这里选择
                  </span>
                </div>
                <div className="vf-settings__row-control">
                  <GlassSelect
                    compact
                    value={cookiesChoice}
                    ariaLabel="登录状态来源"
                    display={COOKIE_OPTIONS.find((o) => o.value === cookiesChoice)?.label}
                    options={COOKIE_OPTIONS.map((o) => ({ key: o.value, label: o.label }))}
                    onChange={(value) => {
                      if (value === "file:") {
                        void chooseCookiesFile();
                        return;
                      }
                      s.setCookiesSource(value);
                    }}
                  />
                  {cookiesFile ? (
                    <button
                      type="button"
                      className="vf-btn-ghost vf-settings__pick"
                      onClick={() => void chooseCookiesFile()}
                      title={cookiesFile}
                    >
                      <IconFolder size={17} />
                      <span className="vf-truncate vf-settings__cookies-path">{cookiesFile}</span>
                    </button>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
        </section>

        <section className="vf-settings__group" aria-labelledby="vf-set-network">
          <h2 id="vf-set-network" className="vf-settings__group-title">
            网络与代理
          </h2>

          <div className="vf-settings__row vf-settings__row--stack">
            <div className="vf-settings__row-label">
              <span>出口模式</span>
              <span className="vf-settings__row-hint">
                决定 VideoFlow 怎么访问站点：解析、下载、XDown 兜底与 yt-dlp 都走这里选定的出口。
                改完立即生效（浏览器解析窗口会在下次解析时重建）
              </span>
            </div>
            <div className="vf-settings__row-control">
              <GlassSelect
                compact
                value={s.proxyMode}
                ariaLabel="网络出口模式"
                options={PROXY_MODES.map((m) => ({ key: m.value, label: m.label }))}
                onChange={(v) => s.setProxyMode(v as ProxyMode)}
              />
            </div>
          </div>

          {s.proxyMode === "custom" ? (
            <div className="vf-settings__row vf-settings__row--stack">
              <div className="vf-settings__row-label">
                <span>代理地址</span>
                <span className="vf-settings__row-hint">
                  {PROXY_TYPES.find((t) => t.value === s.proxyType)?.hint ?? ""}
                </span>
              </div>
              <div className="vf-settings__row-control">
                <GlassSelect
                  compact
                  value={s.proxyType}
                  ariaLabel="代理类型"
                  options={PROXY_TYPES.map((t) => ({ key: t.value, label: t.label }))}
                  onChange={(v) => s.setProxyType(v as ProxyType)}
                />
                <input
                  className={`vf-settings__input${proxyHostError ? " vf-settings__input--invalid" : ""}`}
                  value={proxyHostDraft}
                  aria-label="代理主机与端口"
                  placeholder="127.0.0.1:7890"
                  spellCheck={false}
                  onChange={(e) => setProxyHostDraft(e.target.value)}
                  onBlur={commitProxyHost}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitProxyHost();
                  }}
                />
              </div>
              {proxyHostError ? (
                <span className="vf-settings__cookie-desc vf-settings__cookie-desc--warn">
                  {proxyHostError}
                </span>
              ) : null}
            </div>
          ) : null}

          <div className="vf-settings__row vf-settings__row--stack">
            <div className="vf-settings__row-label">
              <span>网络诊断</span>
              <span className="vf-settings__row-hint">
                分别测本机 DNS、直连、系统代理与自定义代理能不能访问 X
              </span>
            </div>
            <div className="vf-settings__row-control">
              <span className={`vf-settings__cookie-desc vf-settings__cookie-desc--${networkTone}`}>
                {s.networkProbing ? "检测中…" : (s.network?.verdict ?? "尚未检测")}
              </span>
              <button
                type="button"
                className="vf-btn-ghost vf-settings__pick"
                onClick={() => void s.refreshNetwork()}
                disabled={s.networkProbing}
              >
                {s.networkProbing ? "检测中…" : "重新检测"}
              </button>
            </div>
          </div>

          <dl className="vf-diag">
            <div className="vf-diag__row">
              <dt>当前出口</dt>
              <dd className="vf-truncate">
                {s.network
                  ? `${s.network.modeLabel}${s.network.activeProxy ? ` · ${s.network.activeProxy}` : ""}`
                  : "—"}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>本机 DNS</dt>
              <dd className="vf-truncate" title={s.network?.dns.detail}>
                {probeText(s.network?.dns)}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>直连</dt>
              <dd className="vf-truncate" title={s.network?.direct.detail}>
                {probeText(s.network?.direct)}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>系统代理</dt>
              <dd className="vf-truncate" title={s.network?.system.detail}>
                {probeText(s.network?.system)}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>自定义代理</dt>
              <dd className="vf-truncate" title={s.network?.custom.detail}>
                {probeText(s.network?.custom)}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>代理端口</dt>
              <dd className="vf-truncate" title={s.network?.activeProxyEndpoint?.detail}>
                {probeText(s.network?.activeProxyEndpoint)}
              </dd>
            </div>
          </dl>

          <p className="vf-settings__note">
            代理只影响 VideoFlow 自己，不改系统设置。SOCKS 代理下 HLS 任务（FFmpeg 不支持
            SOCKS）与浏览器兜底窗口有限制，遇到问题优先用 HTTP 代理端口或开启 TUN。
          </p>
        </section>

        <section className="vf-settings__group" aria-labelledby="vf-set-appearance">
          <h2 id="vf-set-appearance" className="vf-settings__group-title">
            外观与辅助功能
          </h2>

          <div className="vf-settings__row vf-settings__row--stack">
            <div className="vf-settings__row-label">
              <span>Liquid Glass 渲染档位</span>
              <span className="vf-settings__row-hint">
                当前 {TIER_CODE[s.effectiveTier]} · {s.effectiveReason}
              </span>
            </div>
            <div className="vf-settings__tiers" role="radiogroup" aria-label="渲染档位">
              {TIER_OPTIONS.map((opt) => (
                <button
                  key={opt.value}
                  type="button"
                  role="radio"
                  aria-checked={s.tierChoice === opt.value}
                  className="vf-settings__tier"
                  onClick={() => s.setTierChoice(opt.value)}
                >
                  <span className="vf-settings__tier-label">{opt.label}</span>
                  <span className="vf-settings__tier-hint">{opt.hint}</span>
                </button>
              ))}
            </div>
          </div>

          <div className="vf-settings__row">
            <div className="vf-settings__row-label">
              <span>玻璃强度</span>
              <span className="vf-settings__row-hint">影响乳白着色与光边强度</span>
            </div>
            <div className="vf-settings__row-control">
              <input
                type="range"
                className="vf-slider__input vf-slider__input--inline"
                min={0.4}
                max={1.6}
                step={0.05}
                value={s.glassIntensity}
                aria-label="玻璃强度"
                onChange={(e) => s.setGlassIntensity(Number(e.target.value))}
              />
              <span className="vf-settings__value">{Math.round(s.glassIntensity * 100)}%</span>
            </div>
          </div>

          <div className="vf-settings__row">
            <div className="vf-settings__row-label">
              <span>减少动效</span>
              <span className="vf-settings__row-hint">取消环境扫光与列表进入动画</span>
            </div>
            <div className="vf-settings__row-control">
              <label className="vf-switch">
                <input
                  type="checkbox"
                  checked={s.reducedMotion}
                  aria-label="减少动效"
                  onChange={(e) => s.setReducedMotion(e.target.checked)}
                />
                <span className="vf-switch__track" aria-hidden="true" />
              </label>
            </div>
          </div>

          <div className="vf-settings__row">
            <div className="vf-settings__row-label">
              <span>高对比度</span>
              <span className="vf-settings__row-hint">关闭透明材质，改用实色背板</span>
            </div>
            <div className="vf-settings__row-control">
              <label className="vf-switch">
                <input
                  type="checkbox"
                  checked={s.highContrast}
                  aria-label="高对比度"
                  onChange={(e) => s.setHighContrast(e.target.checked)}
                />
                <span className="vf-switch__track" aria-hidden="true" />
              </label>
            </div>
          </div>
        </section>

        <section className="vf-settings__group" aria-labelledby="vf-set-diag">
          <h2 id="vf-set-diag" className="vf-settings__group-title">
            渲染状态与诊断
          </h2>

          <dl className="vf-diag">
            <div className="vf-diag__row">
              <dt>渲染档位</dt>
              <dd>
                {TIER_CODE[s.effectiveTier]}（{s.tierChoice === "auto" ? "自动推荐" : "手动指定"}）
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>推荐理由</dt>
              <dd>{s.effectiveReason}</dd>
            </div>
            <div className="vf-diag__row">
              <dt>WebGL2</dt>
              <dd>
                {s.profile ? (s.profile.webgl2 ? "可用" : "不可用") : s.profiling ? "检测中…" : "未检测"}
                {s.profile?.floatRenderTarget ? " · 支持浮点渲染目标" : ""}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>图形设备</dt>
              <dd className="vf-truncate" title={s.profile?.rendererName}>
                {s.profile?.rendererName ?? "—"}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>逻辑核心 / 内存</dt>
              <dd>
                {s.profile ? `${s.profile.cores || "—"} 核 / ${s.profile.deviceMemoryGB ?? "不可得"} GB` : "—"}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>帧时间 p95</dt>
              <dd>{s.profile?.frameP95Ms ? `${s.profile.frameP95Ms.toFixed(1)} ms` : "—"}</dd>
            </div>
            <div className="vf-diag__row">
              <dt>yt-dlp</dt>
              <dd className="vf-truncate">
                {s.sidecar?.ytDlp.available
                  ? (s.sidecar.ytDlp.version ?? "已就绪")
                  : sidecarError ?? (s.sidecar ? "未找到（站点解析与站点流下载不可用）" : "检测中…")}
              </dd>
            </div>
            <div className="vf-diag__row">
              <dt>FFmpeg</dt>
              <dd className="vf-truncate">
                {s.sidecar?.ffmpeg.available
                  ? (s.sidecar.ffmpeg.version ?? "已就绪")
                  : sidecarError ?? (s.sidecar ? "未找到（音视频合并不可用）" : "检测中…")}
              </dd>
            </div>
          </dl>

          <p className="vf-settings__note">
            诊断信息只保存在本机，不上传。探测过程不做任何硬件指纹采集。
          </p>
        </section>
      </div>
    </GlassSurface>
  );
}