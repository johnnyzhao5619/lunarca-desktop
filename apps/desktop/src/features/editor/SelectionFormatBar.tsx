// 选区浮动格式条（PRD-06-014 页头收口 P2，Notion / Bear 选区气泡对标）：
// 住在选区气泡条（`SelectionBubble`）**里面**——选区稳定后只出现这一条细气泡，AI 面板
// 由气泡上的 ✦ 或 ⌘J 主动召唤（Notion 同做法），不再一选即弹。条目与动作复用
// `@lunarca/editor-core/editor-actions`（与顶部工具栏、右键菜单同一来源，DRY）；只收高频的行内格式 +
// 链接 + 引用/无序列表，插入类留在顶部工具栏与右键菜单。

import { useTranslation } from "react-i18next";
import { LinkIcon } from "@lunarca/editor-core/Icon";
import {
  formatItemShortcut,
  inlineFormatItems,
  lineBlockItems,
  type EditorFormatAction,
} from "@lunarca/editor-core/editor-actions";
import { BLOCK_ICONS, INLINE_ICONS } from "@lunarca/editor-core/format-icons";
import type { FormatState } from "@lunarca/editor-core/format-commands";

interface SelectionFormatBarProps {
  formatState: FormatState;
  onAction: (action: EditorFormatAction) => void;
}

/** 选区气泡里的块级项只留最常用的两个（引用 / 无序列表）；有序与任务列表在顶部工具栏。 */
const SELECTION_BLOCK_KEYS = new Set(["quote", "bullet"]);

export function SelectionFormatBar({ formatState, onAction }: SelectionFormatBarProps) {
  const { t } = useTranslation("editor-core");

  const title = (labelKey: string, itemKey: string) => {
    const shortcut = formatItemShortcut(itemKey);
    return shortcut ? t("editor.toolbar.labelShortcut", { label: t(labelKey), shortcut }) : t(labelKey);
  };

  return (
    <div className="luna-selection-bubble__format" role="toolbar" aria-label={t("editor.toolbar.label")}>
      {inlineFormatItems(formatState).map((item) => {
        const Icon = INLINE_ICONS[item.key];
        return (
          <button
            key={item.key}
            type="button"
            className="luna-format-toolbar__btn"
            aria-pressed={item.checked}
            title={title(item.labelKey, item.key)}
            aria-label={t(item.labelKey)}
            onClick={() => onAction(item.action)}
          >
            <Icon size={14} />
          </button>
        );
      })}
      <span className="luna-format-toolbar__divider" aria-hidden="true" />
      <button
        type="button"
        className="luna-format-toolbar__btn"
        title={title("editor.menu.insertLink", "link")}
        aria-label={t("editor.menu.insertLink")}
        onClick={() => onAction({ kind: "link" })}
      >
        <LinkIcon size={14} />
      </button>
      {lineBlockItems(formatState)
        .filter((item) => SELECTION_BLOCK_KEYS.has(item.key))
        .map((item) => {
          const Icon = BLOCK_ICONS[item.key];
          return (
            <button
              key={item.key}
              type="button"
              className="luna-format-toolbar__btn"
              aria-pressed={item.checked}
              title={t(item.labelKey)}
              aria-label={t(item.labelKey)}
              onClick={() => onAction(item.action)}
            >
              <Icon size={14} />
            </button>
          );
        })}
    </div>
  );
}
