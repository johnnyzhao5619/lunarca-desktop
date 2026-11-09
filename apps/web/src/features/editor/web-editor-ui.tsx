/**
 * 网页给共享编辑器界面（`@lunarca/editor-core` 的格式工具条与属性面板）的控件原语（`EditorUi`，TDD-17 §2）：
 * 按钮、下拉与输入框用网页自己的控件外观；菜单是一个就地展开的下拉（不定位到坐标——CSP 下不写行内样式，
 * TDD-17 §6.1），键盘与桌面菜单同一种交互：↑↓ 移动、Home / End 到头尾、回车或空格选中、Esc 收起并回到按钮、
 * 点到别处收起。
 */

import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import type { EditorMenuButtonProps, EditorUi } from "@lunarca/editor-core/editor-host";

function WebMenuButton(props: EditorMenuButtonProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLSpanElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  const items = () => [...(menuRef.current?.querySelectorAll<HTMLButtonElement>("[data-menu-item]") ?? [])];

  useEffect(() => {
    if (!open) return;
    items()[0]?.focus();
    const close = (event: PointerEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);

  function onMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const list = items();
    const at = list.indexOf(document.activeElement as HTMLButtonElement);
    const move = (index: number) => {
      event.preventDefault();
      list[(index + list.length) % list.length]?.focus();
    };
    switch (event.key) {
      case "ArrowDown":
        move(at + 1);
        break;
      case "ArrowUp":
        move(at - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(list.length - 1);
        break;
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        triggerRef.current?.focus();
        break;
      case "Tab":
        setOpen(false);
        break;
    }
  }

  const triggerProps = {
    ref: triggerRef,
    type: "button" as const,
    "aria-haspopup": "menu" as const,
    "aria-expanded": open,
    "aria-controls": open ? menuId : undefined,
    onClick: () => setOpen((value) => !value),
  };

  return (
    <span ref={wrapRef} className="luna-web-menu" data-align={props.kind === "icon" ? "end" : "start"}>
      {props.kind === "icon" ? (
        <button {...triggerProps} className="luna-web-icon-button" aria-label={props.label} title={props.label}>
          {props.icon}
        </button>
      ) : (
        <button {...triggerProps} className={props.className} title={props.title}>
          {props.children}
        </button>
      )}
      {open ? (
        <div ref={menuRef} id={menuId} role="menu" aria-label={props.menuLabel} className="luna-web-menu__list" onKeyDown={onMenuKeyDown}>
          {props.entries.map((entry) =>
            entry.kind === "separator" ? (
              <div key={entry.id} role="separator" className="luna-web-menu__separator" />
            ) : (
              <button
                key={entry.id}
                type="button"
                role={entry.checked === undefined ? "menuitem" : "menuitemcheckbox"}
                aria-checked={entry.checked}
                className="luna-web-menu__item"
                tabIndex={-1}
                data-menu-item=""
                onClick={() => {
                  // 焦点归选中的动作安排（格式动作会把焦点交回正文），不还给菜单按钮。
                  setOpen(false);
                  entry.onSelect();
                }}
              >
                <span className="luna-web-menu__icon" aria-hidden="true">
                  {entry.icon}
                </span>
                <span className="luna-web-menu__label">{entry.label}</span>
                {entry.shortcut ? <kbd className="luna-web-menu__shortcut">{entry.shortcut}</kbd> : null}
              </button>
            ),
          )}
        </div>
      ) : null}
    </span>
  );
}

export const webEditorUi: EditorUi = {
  IconButton: ({ label, icon, tone, onClick }) => (
    <button type="button" className="luna-web-icon-button" data-tone={tone} aria-label={label} title={label} onClick={onClick}>
      {icon}
    </button>
  ),
  SelectBox: ({ ariaLabel, value, onChange, children }) => (
    <select className="luna-web-input luna-web-input--sm" aria-label={ariaLabel} value={value} onChange={onChange}>
      {children}
    </select>
  ),
  MenuButton: WebMenuButton,
  inputClassName: "luna-web-input luna-web-input--sm",
  fieldErrorClassName: "luna-web-error",
};
