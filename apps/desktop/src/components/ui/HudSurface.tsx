import { useEffect, useRef, type HTMLAttributes, type ReactNode } from "react";
import { useHudWindowAutoGrow } from "../../hooks/use-hud-window-auto-grow";
import { useWindowDrag } from "../../hooks/use-window-drag";
import {
  NO_COMPOSITION,
  isImeCompositionKey,
  type CompositionState,
} from "../../lib/ime-composition";

type NativeMainProps = Omit<HTMLAttributes<HTMLElement>, "className" | "children">;

export interface HudSurfaceProps extends NativeMainProps {
  /**
   * 窗口身份类（`.luna-popup` 等）：外壳归本组件，feature 类只再声明自己的布局差异。
   * 没有差异的窗不传——快捷对话的圆角与描边收回外壳后，它的身份类就只剩一个空名字。
   */
  className?: string;
  /** 可访问名——五扇窗都是整窗根，必填。 */
  "aria-label": string;
  /**
   * Esc 逃生通道（ADR-111 ⑤⑵）——**必填**。会话中不可关的窗把守卫写进回调（录音的
   * dismiss 经 stateRef 自守卫），分层让路也写在这里（字幕先退沉浸、对话先关历史弹层），
   * 而不是省略本参数：省略意味着这扇窗没有键盘出口。
   */
  onDismiss: () => void;
  /**
   * 内容驱动的向下扩容（TDD-07 §6：档内不缩、仅溢出时增长）。翻译/语音/录音三个内容
   * 驱动 HUD 传 `true` 或带预检高度的对象；快捷对话与实时字幕是稳定可缩放窗口，不传。
   */
  autoGrow?: boolean | { minContentHeight: number };
  /**
   * 当前档位（ADR-112 ③）：变化时 Rust 按该档首帧高 `set_size`（可缩小）并 clamp；档内只扩容。
   * 各窗按会话状态与用户动作（展开 / 收起）给值；不传 = 只扩容不缩。
   */
  tier?: string;
  /**
   * 根元素整面可拖（默认开）。快捷对话与速记传 `false`：它们只由头部拖动，正文要保持
   * 可选中。不再画拖动指示条（ADR-137 V2）——整面可拖时那根条只是在占一行。
   */
  dragRoot?: boolean;
  children: ReactNode;
}

/**
 * HUD 外壳（ADR-111 ②）——五扇悬浮面（弹窗翻译 / 语音速记 / 快捷对话 / 录音 / 实时
 * 字幕）的唯一外壳实现：根几何与表面色、拖动、自动扩容、外壳不可选、Esc 逃生通道。
 *
 * 此前每扇窗各自从原语重拼一遍外壳：四份逐字相同的根声明散在四个 feature 样式里、
 * Esc 各写各的（翻译干脆没有）。feature 从此不再拥有外壳——第六扇窗挂上本组件即自动
 * 长对，样式差异（间距、圆角、透明度）由 feature 类在 features 层覆盖。
 */
export function HudSurface({
  className,
  onDismiss,
  autoGrow = false,
  tier,
  dragRoot = true,
  children,
  ...rest
}: HudSurfaceProps) {
  const containerRef = useRef<HTMLElement | null>(null);
  const { onPointerDown } = useWindowDrag();
  useHudWindowAutoGrow(containerRef, {
    enabled: Boolean(autoGrow),
    minContentHeight: typeof autoGrow === "object" ? autoGrow.minContentHeight : 0,
    tier,
  });

  // onDismiss 走 ref 取最新值：分层让路（沉浸/历史弹层）依赖当前状态，而监听只挂一次。
  const dismissRef = useRef(onDismiss);
  useEffect(() => {
    dismissRef.current = onDismiss;
  });
  // 组字中按 Esc 是在取消候选，不是在关窗：判据只有 `lib/ime-composition.ts` 这一份。
  // 只看 `isComposing` 不够——WKWebView 里 `compositionend` 早于 keydown，那一下 Esc 的
  // isComposing 已是 false，故在窗口上记下组字边界交给同一条判据（时间窗内即属输入法）。
  const compositionRef = useRef<CompositionState>({ ...NO_COMPOSITION });
  useEffect(() => {
    const onCompositionStart = () => {
      compositionRef.current = { active: true, endedAt: compositionRef.current.endedAt };
    };
    const onCompositionEnd = (event: CompositionEvent) => {
      compositionRef.current = { active: false, endedAt: event.timeStamp };
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || isImeCompositionKey(event, compositionRef.current)) return;
      event.preventDefault();
      dismissRef.current();
    };
    window.addEventListener("compositionstart", onCompositionStart, true);
    window.addEventListener("compositionend", onCompositionEnd, true);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("compositionstart", onCompositionStart, true);
      window.removeEventListener("compositionend", onCompositionEnd, true);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  return (
    <main
      {...rest}
      ref={containerRef}
      className={className ? `luna-hud-surface ${className}` : "luna-hud-surface"}
      data-drag-root={dragRoot || undefined}
      onPointerDown={dragRoot ? onPointerDown : undefined}
    >
      {children}
    </main>
  );
}
