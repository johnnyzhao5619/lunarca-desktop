// 上下文菜单呈现层（PRD-07-059）：**唯一一份**菜单面板实现，负责测量定位、
// 键盘导航、子菜单与无障碍语义。它不认识任何业务——条目由各表面以数据形式给出
// （`lib/context-menu.ts`），宿主（`components/ContextMenuHost.tsx`）负责开合。
//
// 定位不再用「估算常量」夹取视口：面板先以不可见状态渲染出真实尺寸，测量后再落位，
// 于是菜单多长都不会顶出屏幕，加减条目也不必回头改常量。

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { cssVariables } from "../../lib/css-variables";
import { CheckIcon, ChevronRightIcon } from "./Icon";
import {
  focusableIndexes,
  menuHasLeadingSlot,
  placeMenu,
  placeSubmenu,
  type ContextMenuItem,
  type ItemRect,
  type MenuPoint,
} from "../../lib/context-menu";

/** 面板落位方式：主菜单锚指针坐标，子菜单锚父项矩形。 */
export type ContextMenuPlacement =
  { kind: "anchor"; point: MenuPoint } | { kind: "submenu"; item: ItemRect };

export interface ContextMenuPanelProps {
  items: ContextMenuItem[];
  placement: ContextMenuPlacement;
  /** 关闭整个菜单（执行条目后、点空白处、Tab 离开）。 */
  onDismiss: () => void;
  /** 只关闭本层（子菜单 Esc / ←）；主菜单传 `onDismiss` 即可。 */
  onCloseSelf: () => void;
  /** 无障碍名称。 */
  label: string;
  /**
   * 挂载即把焦点移入面板。主菜单恒为 true；子菜单**仅键盘展开时**为 true——
   * 悬停展开也抢焦点的话，鼠标用户的焦点会被一路拖着走，而 React 的
   * 「子 effect 先于父 effect」还会让父项把焦点抢回来，形成来回争抢。
   */
  autoFocus: boolean;
}

export function ContextMenuPanel({
  items,
  placement,
  onDismiss,
  onCloseSelf,
  label,
  autoFocus,
}: ContextMenuPanelProps) {
  const panelRef = useRef<HTMLDivElement | null>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [position, setPosition] = useState<MenuPoint | null>(null);
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const [submenu, setSubmenu] = useState<{
    index: number;
    item: ItemRect;
    viaKeyboard: boolean;
  } | null>(null);

  // 测量真实尺寸后落位（未落位前面板不可见，避免左上角闪一下再跳过去）。
  //
  // 落位结果**同值即不写回**：`placeMenu` 每次返回新对象，若无脑 setState 就会
  // 「effect → 新 state → 重渲染 → effect」自锁成死循环（本批次自审时踩到）。
  useLayoutEffect(() => {
    const panel = panelRef.current;
    if (!panel) {
      return;
    }
    const rect = panel.getBoundingClientRect();
    const size = { width: rect.width, height: rect.height };
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const next =
      placement.kind === "anchor"
        ? placeMenu(placement.point, size, viewport)
        : placeSubmenu(placement.item, size, viewport);
    setPosition((current) =>
      current && current.x === next.x && current.y === next.y ? current : next,
    );
  }, [placement, items]);

  // 打开即把焦点移进面板：Esc 才有落点，方向键才接得住（键盘可达，PRD-00 §7 a11y）。
  // `preventScroll` 是硬要求——聚焦引发的滚动会立刻触发宿主的"滚动即关闭"。
  //
  // **必须等落位之后再聚焦**（真机 2026-09-02：Tab 到页头 ⋯、Enter 打开菜单后方向键不动、
  // Esc 关不掉、焦点也回不到按钮）：未落位的面板 `data-placed="false"` 是不可见的，浏览器对
  // 不可见元素的 `focus()` 静默不做；而上面那条 layout effect 里的 `setPosition` 会让 React
  // 在同步重渲染**之前**先冲掉本 effect——于是它恰好在面板还看不见的那一帧跑，焦点留在原处。
  // 依赖上"已落位"，并用 layout effect 在绘制前聚焦，两个时序问题一起收掉。
  const placed = position !== null;
  useLayoutEffect(() => {
    if (autoFocus && placed) {
      panelRef.current?.focus({ preventScroll: true });
    }
  }, [autoFocus, placed]);

  useEffect(() => {
    if (activeIndex !== null) {
      itemRefs.current[activeIndex]?.focus({ preventScroll: true });
    }
  }, [activeIndex]);

  function moveActive(step: 1 | -1) {
    const focusable = focusableIndexes(items);
    if (focusable.length === 0) {
      return;
    }
    const current = activeIndex === null ? -1 : focusable.indexOf(activeIndex);
    const next =
      current === -1
        ? step === 1
          ? 0
          : focusable.length - 1
        : (current + step + focusable.length) % focusable.length;
    setSubmenu(null);
    setActiveIndex(focusable[next]);
  }

  function activate(item: ContextMenuItem, index: number) {
    if (item.kind === "separator" || item.kind === "heading" || item.disabled) {
      return;
    }
    if (item.kind === "submenu") {
      openSubmenu(index, true);
      return;
    }
    // 先关菜单再执行：动作可能弹确认框，菜单浮在上面会挡住它。
    onDismiss();
    item.onSelect();
  }

  function openSubmenu(index: number, viaKeyboard: boolean) {
    const button = itemRefs.current[index];
    if (!button) {
      return;
    }
    const rect = button.getBoundingClientRect();
    setActiveIndex(index);
    setSubmenu({
      index,
      item: { left: rect.left, right: rect.right, top: rect.top },
      viaKeyboard,
    });
  }

  const submenuItem = submenu === null ? null : items[submenu.index];
  // 同层任一条目带图标/勾选 → 所有条目渲染定宽前导槽，文字左缘对齐（按层判定）。
  const hasLeading = menuHasLeadingSlot(items);

  return (
    <>
      <div
        ref={panelRef}
        className="luna-context-menu"
        role="menu"
        aria-label={label}
        tabIndex={-1}
        data-placed={position !== null ? "true" : "false"}
        style={
          position === null
            ? undefined
            : cssVariables({ "--menu-x": `${position.x}px`, "--menu-y": `${position.y}px` })
        }
        onMouseDown={(event) => event.stopPropagation()}
        onContextMenu={(event) => event.preventDefault()}
        onKeyDown={(event) => {
          switch (event.key) {
            case "ArrowDown":
              event.preventDefault();
              moveActive(1);
              break;
            case "ArrowUp":
              event.preventDefault();
              moveActive(-1);
              break;
            case "Home":
              event.preventDefault();
              setActiveIndex(focusableIndexes(items)[0] ?? null);
              break;
            case "End": {
              event.preventDefault();
              const focusable = focusableIndexes(items);
              setActiveIndex(focusable[focusable.length - 1] ?? null);
              break;
            }
            case "ArrowRight":
              if (activeIndex !== null && items[activeIndex]?.kind === "submenu") {
                event.preventDefault();
                openSubmenu(activeIndex, true);
              }
              break;
            case "ArrowLeft":
              event.preventDefault();
              if (submenu !== null) {
                setSubmenu(null);
              } else {
                onCloseSelf();
              }
              break;
            case "Escape":
              event.preventDefault();
              onCloseSelf();
              break;
            case "Tab":
              event.preventDefault();
              onDismiss();
              break;
            default:
              break;
          }
        }}
      >
        {items.map((item, index) => {
          if (item.kind === "separator") {
            return <div key={item.id} className="luna-context-menu__separator" role="separator" />;
          }
          if (item.kind === "heading") {
            // 分组标题只是说明：不进焦点环、不作菜单项（读屏按分组名念出，不念成可点的项）。
            return (
              <div key={item.id} className="luna-context-menu__heading" role="presentation">
                {item.label}
              </div>
            );
          }
          const isSubmenu = item.kind === "submenu";
          const checkable = item.kind === "action" && item.checked !== undefined;
          return (
            <button
              key={item.id}
              ref={(element) => {
                itemRefs.current[index] = element;
              }}
              type="button"
              role={checkable ? "menuitemcheckbox" : "menuitem"}
              aria-checked={checkable ? item.checked : undefined}
              aria-haspopup={isSubmenu ? "menu" : undefined}
              aria-expanded={isSubmenu ? submenu?.index === index : undefined}
              className={
                item.kind === "action" && item.danger
                  ? "luna-context-menu__item luna-context-menu__danger"
                  : "luna-context-menu__item"
              }
              tabIndex={-1}
              disabled={item.disabled}
              title={item.kind === "action" ? item.title : undefined}
              data-active={activeIndex === index || undefined}
              onMouseEnter={() => {
                if (item.disabled) {
                  return;
                }
                setActiveIndex(index);
                if (isSubmenu) {
                  openSubmenu(index, false);
                } else {
                  setSubmenu(null);
                }
              }}
              onClick={() => activate(item, index)}
            >
              {checkable ? (
                <span className="luna-context-menu__checkslot" aria-hidden="true">
                  {item.kind === "action" && item.checked ? <CheckIcon size={12} /> : null}
                </span>
              ) : item.icon ? (
                <span className="luna-context-menu__icon" aria-hidden="true">
                  {item.icon}
                </span>
              ) : hasLeading ? (
                <span className="luna-context-menu__checkslot" aria-hidden="true" />
              ) : null}
              <span className="luna-context-menu__label">{item.label}</span>
              {item.kind === "action" && item.shortcut ? (
                <span className="luna-context-menu__shortcut">{item.shortcut}</span>
              ) : null}
              {isSubmenu ? (
                <ChevronRightIcon size={12} className="luna-context-menu__chevron" />
              ) : null}
            </button>
          );
        })}
      </div>
      {submenu && submenuItem?.kind === "submenu" ? (
        <ContextMenuPanel
          items={submenuItem.items}
          placement={{ kind: "submenu", item: submenu.item }}
          onDismiss={onDismiss}
          onCloseSelf={() => {
            setSubmenu(null);
            setActiveIndex(submenu.index);
          }}
          label={submenuItem.label}
          autoFocus={submenu.viaKeyboard}
        />
      ) : null}
    </>
  );
}
