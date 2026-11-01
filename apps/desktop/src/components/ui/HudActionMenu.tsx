import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { IconButton } from "./IconButton";
import { CheckIcon, EllipsisIcon } from "./Icon";

export type HudActionMenuItem = {
  id: string;
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
  tone?: "default" | "danger";
  checked?: boolean;
  onSelect: () => void;
};

/** HUD 菜单的方向键目标：跳过禁用项并在首尾循环。 */
export function nextHudMenuItemIndex(
  items: readonly HudActionMenuItem[],
  current: number | null,
  step: 1 | -1,
): number | null {
  const enabled = items.flatMap((item, index) => (item.disabled ? [] : [index]));
  if (enabled.length === 0) return null;
  const position = current === null ? -1 : enabled.indexOf(current);
  if (position === -1) return step === 1 ? enabled[0] : enabled[enabled.length - 1];
  return enabled[(position + step + enabled.length) % enabled.length];
}

type HudActionMenuProps = {
  label: string;
  items: readonly HudActionMenuItem[];
  align?: "start" | "end";
  side?: "top" | "bottom";
  /**
   * 这是本窗的「⋯」主菜单：⌘K（Windows 为 Ctrl+K）开合它（ADR-137 V3/V8，与 Raycast 的
   * 动作面板同一心智）。一扇窗只许一份——两份同时挂上，同一组合键就有两个去处。
   * 主窗口里 ⌘K 是命令面板，嵌在主窗口里的同一视图不得打开它。
   */
  shortcut?: boolean;
  /** 触发钮的密度：默认 HUD 档；正文行内（消息悬停动作）用 `sm`，与同排的图标钮一样大。 */
  density?: "sm" | "hud";
};

/** ⌘K / Ctrl+K：只认这一个组合，不带 Shift/Alt（那些留给别的键位）。 */
export function isHudMenuShortcut(event: KeyboardEvent): boolean {
  return (
    (event.metaKey || event.ctrlKey) &&
    !event.shiftKey &&
    !event.altKey &&
    !event.isComposing &&
    event.key.toLowerCase() === "k"
  );
}

/**
 * HUD 的统一低频动作入口。菜单只管理瞬时开合与键盘/外点关闭；动作语义仍由调用方拥有。
 */
export function HudActionMenu({
  label,
  items,
  align = "end",
  side = "top",
  shortcut = false,
  density = "hud",
}: HudActionMenuProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);

  const closeAndRestoreFocus = useCallback(() => {
    setOpen(false);
    setActiveIndex(null);
    triggerRef.current?.focus({ preventScroll: true });
  }, []);

  function openAt(index: number | null) {
    setActiveIndex(index);
    setOpen(true);
  }

  // 开合走 ref 取最新值：监听只挂一次，而开没开着每次都在变。
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    if (!shortcut) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isHudMenuShortcut(event)) return;
      event.preventDefault();
      if (openRef.current) {
        closeAndRestoreFocus();
      } else {
        setActiveIndex(null);
        setOpen(true);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeAndRestoreFocus, shortcut]);

  useEffect(() => {
    if (!open) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
        setActiveIndex(null);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeAndRestoreFocus();
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer, true);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer, true);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [closeAndRestoreFocus, open]);

  useEffect(() => {
    if (!open) return;
    const first = nextHudMenuItemIndex(items, null, 1);
    setActiveIndex((current) =>
      current !== null && items[current] && !items[current].disabled ? current : first,
    );
  }, [items, open]);

  useEffect(() => {
    if (open && activeIndex !== null) {
      itemRefs.current[activeIndex]?.focus({ preventScroll: true });
    }
  }, [activeIndex, open]);

  const hasLeadingSlot = items.some((item) => item.icon !== undefined);

  return (
    <div
      ref={rootRef}
      className="luna-hud-menu"
      data-align={align}
      data-side={side}
      data-no-drag
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
          setActiveIndex(null);
        }
      }}
    >
      <IconButton
        ref={triggerRef}
        density={density}
        label={label}
        icon={<EllipsisIcon size={density === "sm" ? 12 : 16} />}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-keyshortcuts={shortcut ? "Meta+K Control+K" : undefined}
        onClick={() => {
          if (open) {
            setOpen(false);
            setActiveIndex(null);
          } else {
            openAt(null);
          }
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            openAt(nextHudMenuItemIndex(items, null, event.key === "ArrowDown" ? 1 : -1));
          }
        }}
      />
      {open ? (
        <div
          className="luna-hud-menu__popover"
          role="menu"
          aria-label={label}
          onKeyDown={(event) => {
            switch (event.key) {
              case "ArrowDown":
              case "ArrowUp":
                event.preventDefault();
                event.stopPropagation();
                setActiveIndex((current) =>
                  nextHudMenuItemIndex(items, current, event.key === "ArrowDown" ? 1 : -1),
                );
                break;
              case "Home":
                event.preventDefault();
                event.stopPropagation();
                setActiveIndex(nextHudMenuItemIndex(items, null, 1));
                break;
              case "End":
                event.preventDefault();
                event.stopPropagation();
                setActiveIndex(nextHudMenuItemIndex(items, null, -1));
                break;
              case "Escape":
                event.preventDefault();
                event.stopPropagation();
                closeAndRestoreFocus();
                break;
              default:
                break;
            }
          }}
        >
          {items.map((item, index) => (
            <button
              key={item.id}
              ref={(element) => {
                itemRefs.current[index] = element;
              }}
              type="button"
              role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
              aria-checked={item.checked === undefined ? undefined : item.checked}
              className="luna-hud-menu__item"
              data-tone={item.tone ?? "default"}
              disabled={item.disabled}
              tabIndex={-1}
              onFocus={() => setActiveIndex(index)}
              onClick={() => {
                item.onSelect();
                setOpen(false);
                setActiveIndex(null);
              }}
            >
              {item.icon ? (
                <span className="luna-hud-menu__item-icon" aria-hidden="true">
                  {item.icon}
                </span>
              ) : hasLeadingSlot ? (
                <span className="luna-hud-menu__item-icon" aria-hidden="true" />
              ) : null}
              <span className="luna-hud-menu__item-label">{item.label}</span>
              {item.checked !== undefined ? (
                <span className="luna-hud-menu__item-check" aria-hidden="true">
                  {item.checked ? <CheckIcon size={14} /> : null}
                </span>
              ) : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
