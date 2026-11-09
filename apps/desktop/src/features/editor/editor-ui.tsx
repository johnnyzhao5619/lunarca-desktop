/**
 * 桌面给共享编辑器界面（`@lunarca/editor-core` 的格式工具条与属性面板）的控件原语（`EditorUi`，TDD-17 §2）：
 * 按钮、下拉、输入框用全应用唯一的那几个原语，菜单交给全应用唯一的菜单宿主（`ContextMenuHost`，PRD-07-059）。
 */

import type { EditorMenuButtonProps, EditorMenuEntry, EditorUi } from "@lunarca/editor-core/editor-host";
import { useContextMenu } from "../../components/ContextMenuHost";
import { IconButton } from "../../components/ui/IconButton";
import { SelectBox } from "../../components/ui/SelectBox";
import { buildMenu, separator, type ContextMenuItem } from "../../lib/context-menu";

function menuItems(entries: readonly EditorMenuEntry[]): ContextMenuItem[] {
  return buildMenu(
    entries.map((entry) =>
      entry.kind === "separator"
        ? separator(entry.id)
        : {
            kind: "action" as const,
            id: entry.id,
            label: entry.label,
            checked: entry.checked,
            shortcut: entry.shortcut,
            icon: entry.icon,
            onSelect: entry.onSelect,
          },
    ),
  );
}

function MenuButton(props: EditorMenuButtonProps) {
  const contextMenu = useContextMenu();
  const options = props.menuLabel ? { label: props.menuLabel } : undefined;
  if (props.kind === "icon") {
    return (
      <IconButton
        density="sm"
        label={props.label}
        icon={props.icon}
        aria-haspopup="menu"
        onClick={(event) => contextMenu.open(event, menuItems(props.entries), options)}
      />
    );
  }
  return (
    <button
      type="button"
      className={props.className}
      aria-haspopup="menu"
      title={props.title}
      onClick={(event) => contextMenu.open(event, menuItems(props.entries), options)}
    >
      {props.children}
    </button>
  );
}

export const desktopEditorUi: EditorUi = {
  IconButton: ({ label, icon, tone, onClick }) => <IconButton density="sm" tone={tone} label={label} icon={icon} onClick={onClick} />,
  SelectBox: ({ ariaLabel, value, onChange, children }) => (
    <SelectBox size="sm" ariaLabel={ariaLabel} value={value} onChange={onChange}>
      {children}
    </SelectBox>
  ),
  MenuButton,
  inputClassName: "luna-input luna-input--sm",
  fieldErrorClassName: "luna-field-error",
};
