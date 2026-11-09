import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { cssVariables } from "../../lib/css-variables";
import { HelpCircleIcon } from "@lunarca/editor-core/Icon";
import { IconButton } from "./IconButton";

const useMeasuredLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export interface HelpDetailsProps {
  /** 说明浮层的标题与问号按钮名称，须指明对应对象。 */
  summary: string;
  children: ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

/**
 * PRD-09-085：点击问号读取简短的概念说明，不增加正文高度。流程与技术资料使用具名详情，
 * 当前错误和必要后果由调用方常显。Portal 避免被设置页的滚动容器裁切；
 * 实测位置只经 cssVariables 注入，视觉与尺寸仍由归属 CSS / Token 决定。
 */
export function HelpDetails({ summary, children, open: controlledOpen, onOpenChange }: HelpDetailsProps) {
  const [localOpen, setLocalOpen] = useState(false);
  const open = controlledOpen ?? localOpen;
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ x: number; y: number } | null>(null);
  const setOpenRef = useRef((next: boolean) => {
    setLocalOpen(next);
    onOpenChange?.(next);
  });
  setOpenRef.current = (next) => { setLocalOpen(next); onOpenChange?.(next); };

  useMeasuredLayoutEffect(() => {
    if (!open || !panel.current || !trigger.current) return;
    const anchor = trigger.current.getBoundingClientRect();
    const rect = panel.current.getBoundingClientRect();
    const style = getComputedStyle(panel.current);
    const gap = parseFloat(style.getPropertyValue("--space-2"));
    const edge = parseFloat(style.getPropertyValue("--space-3"));
    const left = Math.max(edge, Math.min(anchor.left, window.innerWidth - rect.width - edge));
    const x = style.direction === "rtl" ? window.innerWidth - left - rect.width : left;
    const above = anchor.top - rect.height - gap;
    const below = anchor.bottom + gap;
    const preferred = above >= edge ? above : below;
    const y = Math.max(edge, Math.min(preferred, window.innerHeight - rect.height - edge));
    setPosition((current) => current?.x === x && current.y === y ? current : { x, y });
  }, [open, children]);

  useEffect(() => {
    if (!open) { setPosition(null); return; }
    document.dispatchEvent(new CustomEvent("luna:help-open", { detail: id }));
    const close = () => setOpenRef.current(false);
    const outside = (event: PointerEvent) => {
      if (!trigger.current?.contains(event.target as Node) && !panel.current?.contains(event.target as Node)) close();
    };
    const other = (event: Event) => { if ((event as CustomEvent<string>).detail !== id) close(); };
    const scroll = (event: Event) => { if (!panel.current?.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.stopPropagation(); close(); trigger.current?.focus({ preventScroll: true }); }
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("luna:help-open", other);
    document.addEventListener("scroll", scroll, true);
    document.addEventListener("keydown", escape);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("luna:help-open", other);
      document.removeEventListener("scroll", scroll, true);
      document.removeEventListener("keydown", escape);
      window.removeEventListener("resize", close);
    };
  }, [open, id]);

  return (
    <span className="luna-help-details">
      <IconButton ref={trigger} label={summary} icon={<HelpCircleIcon size="1em" />} className="luna-help-details__trigger"
        aria-expanded={open} aria-controls={open ? id : undefined} aria-haspopup="dialog"
        onClick={() => setOpenRef.current(!open)} />
      {open ? createPortal(
        <div ref={panel} id={id} role="dialog" aria-label={summary} className="luna-help-details__popover"
          data-placed={position !== null} style={cssVariables({ "--help-x": position ? `${position.x}px` : undefined,
            "--help-y": position ? `${position.y}px` : undefined })}>
          <strong className="luna-help-details__title">{summary}</strong>
          <div className="luna-help-details__body">{children}</div>
        </div>, document.body,
      ) : null}
    </span>
  );
}
