/**
 * URL 输入胶囊（项目书 §2.3 UrlComposer）。
 * 状态：idle / focus / invalid / parsing / parsed
 * Enter 解析，Esc 清空或关闭结果。
 *
 * 聚焦时展开链接历史（最多 10 条），点击填入输入框但**不自动解析**——
 * 历史里有相似链接时自动解析会把用户没确认的东西直接送出去。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { GlassSurface } from "@/components/glass/GlassSurface";
import { IconClose, IconLink } from "@/components/icons";
import { relativeTime, useHistoryStore, VISIBLE_LIMIT } from "@/stores/historyStore";
import { checkUrl } from "./urlValidation";

export interface UrlComposerProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: (normalizedUrl: string) => void;
  onCancel: () => void;
  parsing: boolean;
  disabled?: boolean;
  /** 主进程返回的分类错误 */
  externalError?: string | null;
}

const PLACEHOLDER = "粘贴视频链接，或整段抖音分享文案";

export function UrlComposer({
  value,
  onChange,
  onSubmit,
  onCancel,
  parsing,
  disabled = false,
  externalError = null,
}: UrlComposerProps): React.JSX.Element {
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [focus, setFocus] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  /** 历史下拉是否已被用户主动关掉（Esc 或选中某项之后） */
  const [historyClosed, setHistoryClosed] = useState(false);

  const history = useHistoryStore((s) => s.entries);

  // 输入变化即清除上一次的错误，避免「改完还报错」
  useEffect(() => {
    setLocalError(null);
  }, [value]);

  /**
   * 展示的历史：有输入时按输入过滤，避免一长串历史挡住视线。
   * 输入框为空时给最近若干条，让「再下一次上次那个」一步可达。
   */
  const visibleHistory = useMemo(() => {
    const q = value.trim().toLowerCase();
    const list = q
      ? history.filter(
          (h) => h.url.toLowerCase().includes(q) || h.title.toLowerCase().includes(q),
        )
      : history;
    return list.slice(0, VISIBLE_LIMIT);
  }, [history, value]);

  const showHistory =
    focus && !historyClosed && !parsing && !disabled && visibleHistory.length > 0;

  const error = localError ?? externalError;
  const canSubmit = value.trim().length > 0 && !parsing && !disabled;

  const submit = () => {
    if (parsing) return;
    const result = checkUrl(value);
    if (!result.ok) {
      setLocalError(result.issue?.message ?? "链接无效");
      return;
    }
    setLocalError(null);
    setHistoryClosed(true);
    // 粘整段分享文案时，把输入框收敛成真正要解析的那条链接，
    // 免得用户以为后面那串说明文字也会被拿去解析
    if (result.normalized !== value) onChange(result.normalized);
    onSubmit(result.normalized);
  };

  /** 选中历史项：只填入，不自动解析 */
  const chooseHistory = (url: string) => {
    onChange(url);
    setHistoryClosed(true);
    setLocalError(null);
    inputRef.current?.focus();
  };

  const removeHistory = async (url: string, e: React.MouseEvent) => {
    e.stopPropagation();
    await useHistoryStore.getState().remove(url);
  };

  return (
    <div className="vf-composer">
      <GlassSurface
        variant="control"
        className={
          error
            ? "vf-composer__field is-invalid"
            : focus
              ? "vf-composer__field is-focus"
              : "vf-composer__field"
        }
        tint={0.34}
        opacity={0.72}
      >
        <span className="vf-composer__icon" aria-hidden="true">
          <IconLink size={20} />
        </span>
        <input
          ref={inputRef}
          className="vf-composer__input"
          type="text"
          value={value}
          placeholder={PLACEHOLDER}
          spellCheck={false}
          autoComplete="off"
          disabled={disabled || parsing}
          aria-label="视频链接"
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? "vf-composer-error" : undefined}
          onChange={(e) => {
            onChange(e.target.value);
            setHistoryClosed(false);
          }}
          onFocus={() => {
            setFocus(true);
            setHistoryClosed(false);
          }}
          onBlur={() => setFocus(false)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              if (showHistory) {
                // 先收下拉，再谈清空——否则用户想关下拉却把输入也清了
                setHistoryClosed(true);
              } else if (value) {
                onChange("");
              } else {
                inputRef.current?.blur();
              }
            }
          }}
        />
        {value && !parsing ? (
          <button
            type="button"
            className="vf-composer__clear"
            aria-label="清空输入"
            onClick={() => {
              onChange("");
              setHistoryClosed(false);
              inputRef.current?.focus();
            }}
          >
            <IconClose size={15} />
          </button>
        ) : null}
      </GlassSurface>

      <button
        type="button"
        className="vf-btn-primary vf-composer__submit"
        disabled={!canSubmit}
        onClick={parsing ? onCancel : submit}
      >
        {parsing ? "取消" : "解析"}
      </button>

      {showHistory ? (
        <div className="vf-history" role="listbox" aria-label="链接历史">
          {visibleHistory.map((h) => (
            <div key={h.url} className="vf-history__item">
              <button
                type="button"
                className="vf-history__pick"
                role="option"
                aria-selected={false}
                title={h.url}
                /* 用 mousedown 而不是 click：click 之前输入框已经 blur，下拉会先消失 */
                onMouseDown={(e) => {
                  e.preventDefault();
                  chooseHistory(h.url);
                }}
              >
                <span className="vf-history__title vf-truncate">{h.title || h.url}</span>
                <span className="vf-history__meta">
                  {h.sourceName ? `${h.sourceName} · ` : ""}
                  {relativeTime(h.at)}
                </span>
              </button>
              <button
                type="button"
                className="vf-history__remove"
                aria-label={`删除历史：${h.title || h.url}`}
                onMouseDown={(e) => e.preventDefault()}
                onClick={(e) => void removeHistory(h.url, e)}
              >
                <IconClose size={13} />
              </button>
            </div>
          ))}
          <button
            type="button"
            className="vf-history__clear"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => void useHistoryStore.getState().clear()}
          >
            清空历史
          </button>
        </div>
      ) : null}

      {error ? (
        <p id="vf-composer-error" className="vf-composer__error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}