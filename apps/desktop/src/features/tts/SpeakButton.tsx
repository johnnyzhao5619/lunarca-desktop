// 发音/朗读按钮（HUD 弹窗原文/译文、以及任意「短文本朗读」表面复用）。
//
// 单一发声者语义：点击→朗读并记住返回的 `session_id`；再次点击（或朗读中）→停止。跨窗口若被
// 别处朗读抢占，本按钮据 `session_id` 不匹配自动复位为空闲态。不支持的平台整块隐藏。

import { useState } from "react";

import { SpeakIcon, SpeakingIcon } from "@lunarca/editor-core/Icon";
import { useTts, useTtsAvailable } from "./use-tts";

type SpeakButtonProps = {
  /** 要朗读的文本。 */
  text: string;
  /** 显式语言（BCP-47 或 `zh`/`en`；省略/`null` = 后端本地检测）。 */
  lang?: string | null;
  /** 发起表面标识（随状态回传供认领）。 */
  scope: string;
  /** 是否按 Markdown 剥离（HUD 纯文本默认 false）。 */
  markdown?: boolean;
  /** 可见文案（省略则纯图标）。 */
  label?: string;
  /** 无障碍标签 / tooltip（朗读态与空闲态各一，通过 `label`/`activeLabel` 提供）。 */
  title: string;
  /** 朗读中的无障碍标签（省略则复用 `title`）。 */
  activeTitle?: string;
  /** 附加类名（默认 `luna-speak-btn`，HUD 传 `luna-icon-btn luna-icon-btn--hud` 等对齐周边按钮）。 */
  className?: string;
};

export function SpeakButton({
  text,
  lang = null,
  scope,
  markdown = false,
  label,
  title,
  activeTitle,
  className = "luna-speak-btn",
}: SpeakButtonProps) {
  const available = useTtsAvailable();
  const { state, speak, stop } = useTts();
  const [sessionId, setSessionId] = useState(0);

  if (!available) return null;

  const isMine = sessionId !== 0 && state.session_id === sessionId;
  const active = isMine && state.status !== "idle";
  const disabled = text.trim().length === 0;

  const onClick = async () => {
    if (active) {
      await stop();
      return;
    }
    const next = await speak({ text, markdown, lang, scope });
    setSessionId(next.session_id);
  };

  return (
    <button
      type="button"
      className={className}
      data-speaking={active}
      aria-pressed={active}
      disabled={disabled}
      title={active ? (activeTitle ?? title) : title}
      aria-label={active ? (activeTitle ?? title) : title}
      onClick={() => {
        void onClick();
      }}
    >
      {active ? <SpeakingIcon size={14} /> : <SpeakIcon size={14} />}
      {label ? <span>{label}</span> : null}
    </button>
  );
}
