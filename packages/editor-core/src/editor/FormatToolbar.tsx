// 格式快捷编辑栏（PRD-06-014，Typora/Apple Notes 对标；网页同一条，PRD-21-020）：帮助不熟悉 Markdown 的
// 用户可视化施加格式——标题下拉 + 行内格式（激活态高亮）+ 列表/引用 + 链接 + 插入（表格/代码块/分隔线）。
// 条目与动作复用 `editor-actions`（与右键菜单、斜杠菜单同一来源）；仅编辑型模式渲染（阅读模式由宿主决定
// 不挂载）。菜单怎么开、图标按钮长什么样归宿主的控件原语（`EditorUi`）。
//
// 快捷键提示一律经 `formatItemShortcut` 从命令注册表派生，不写死任何组合。

import type { MouseEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  CalloutIcon,
  ChevronDownIcon,
  CodeSquareIcon,
  EllipsisIcon,
  HeadingIcon,
  ImageIcon,
  LinkIcon,
  MinusIcon,
  TableIcon,
  WikiLinkIcon,
} from "../ui/Icon";
import { BLOCK_ICONS, INLINE_ICONS } from "./format-icons";
import {
  formatItemShortcut,
  headingItems,
  inlineFormatItems,
  lineBlockItems,
  type EditorFormatAction,
} from "./editor-actions";
import type { EditorMenuEntry, EditorUi } from "./editor-host";
import type { FormatState } from "./format-commands";

export interface FormatToolbarProps {
  formatState: FormatState;
  onAction: (action: EditorFormatAction) => void;
  ui: EditorUi;
  /**
   * 编辑器是不是宽档（桌面 `editor-tier.ts` 的 `wide`）：非宽档时「图片 / 附件」也收进尾部的「更多格式」
   * 菜单——高频的段落 / 行内 / 块级按钮留在面上，低频的插入类先让位。
   */
  wide: boolean;
  /** 宿主能不能「插入图片 / 附件」（选文件、存进库归宿主）；不能就不出这一项。 */
  attachments: boolean;
}

/** 「插入」组：无勾选态的一次性动作（图片/附件 / 表格 / 代码块 / 标注块 / 分隔线）。 */
const INSERT_ITEMS: readonly { key: string; labelKey: string; action: EditorFormatAction; Icon: typeof TableIcon }[] = [
  { key: "attachment", labelKey: "editor.menu.insertAttachment", action: { kind: "attachment" }, Icon: ImageIcon },
  { key: "table", labelKey: "editor.table.insert", action: { kind: "table" }, Icon: TableIcon },
  { key: "codeBlock", labelKey: "editor.menu.codeBlock", action: { kind: "codeBlock" }, Icon: CodeSquareIcon },
  { key: "callout", labelKey: "editor.menu.callout", action: { kind: "callout" }, Icon: CalloutIcon },
  { key: "hr", labelKey: "editor.menu.hr", action: { kind: "hr" }, Icon: MinusIcon },
];

/** 留在面上的插入类：宽档两格（图片 / 附件、表格），窄档只留表格。 */
const DIRECT_INSERT_KEYS: readonly string[] = ["attachment", "table"];

/** 按下不夺走编辑器的焦点与选区（浏览器里点按钮会把焦点挪到按钮上）。 */
function keepEditorFocus(event: MouseEvent) {
  event.preventDefault();
}

export function FormatToolbar({ formatState, onAction, ui, wide, attachments }: FormatToolbarProps) {
  const { t } = useTranslation("editor-core");
  const { MenuButton } = ui;
  const offered = INSERT_ITEMS.filter((item) => attachments || item.key !== "attachment");
  const direct = (key: string) => DIRECT_INSERT_KEYS.includes(key) && (wide || key === "table");

  const headings = headingItems(formatState);
  const currentHeading = headings.find((item) => item.checked && item.key !== "text") ?? null;
  const headingLabel = currentHeading ? t(currentHeading.labelKey) : t("editor.menu.paragraphText");

  /** 工具提示 = 名称（快捷键）；没绑键就只有名称。 */
  const title = (labelKey: string, itemKey: string) => {
    const shortcut = formatItemShortcut(itemKey);
    return shortcut ? t("editor.toolbar.labelShortcut", { label: t(labelKey), shortcut }) : t(labelKey);
  };

  const headingEntries: EditorMenuEntry[] = headings.map((item) => ({
    kind: "action",
    id: item.key,
    label: t(item.labelKey),
    checked: item.checked,
    shortcut: formatItemShortcut(item.key),
    onSelect: () => onAction(item.action),
  }));

  const moreEntries: EditorMenuEntry[] = [
    {
      kind: "action",
      id: "wikiLink",
      label: t("editor.menu.insertWikiLink"),
      icon: <WikiLinkIcon size={13} />,
      onSelect: () => onAction({ kind: "wikiLink" }),
    },
    { kind: "separator", id: "insert" },
    ...offered
      .filter((item) => !direct(item.key))
      .map(({ key, labelKey, action, Icon }): EditorMenuEntry => ({
        kind: "action",
        id: key,
        label: t(labelKey),
        icon: <Icon size={13} />,
        shortcut: formatItemShortcut(key),
        onSelect: () => onAction(action),
      })),
  ];

  return (
    <div className="luna-format-toolbar" role="toolbar" aria-label={t("editor.toolbar.label")}>
      <MenuButton kind="text" className="luna-format-toolbar__heading-trigger" title={t("editor.toolbar.headingTitle")} entries={headingEntries}>
        <HeadingIcon size={13} />
        <span className="luna-format-toolbar__heading-label">{headingLabel}</span>
        <ChevronDownIcon size={11} />
      </MenuButton>
      <span className="luna-format-toolbar__divider" aria-hidden="true" />
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
            onMouseDown={keepEditorFocus}
            onClick={() => onAction(item.action)}
          >
            <Icon size={14} />
          </button>
        );
      })}
      <span className="luna-format-toolbar__divider" aria-hidden="true" />
      {lineBlockItems(formatState).map((item) => {
        const Icon = BLOCK_ICONS[item.key];
        return (
          <button
            key={item.key}
            type="button"
            className="luna-format-toolbar__btn"
            aria-pressed={item.checked}
            title={title(item.labelKey, item.key)}
            aria-label={t(item.labelKey)}
            onMouseDown={keepEditorFocus}
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
        onMouseDown={keepEditorFocus}
        onClick={() => onAction({ kind: "link" })}
      >
        <LinkIcon size={14} />
      </button>
      {/* 直接动作（R5，2026-10-08）：宽档多给「图片 / 附件」一格，表格恒在面上；其余插入类
          （wiki 链接、代码块、标注块、分隔线）在**所有宽度**都收进同一个「更多格式」菜单——
          按工具组固定分组，不量字宽猜重要性；快捷键照旧可达。 */}
      {offered
        .filter((item) => direct(item.key))
        .map(({ key, labelKey, action, Icon }) => (
          <button
            key={key}
            type="button"
            className="luna-format-toolbar__btn"
            title={title(labelKey, key)}
            aria-label={t(labelKey)}
            onMouseDown={keepEditorFocus}
            onClick={() => onAction(action)}
          >
            <Icon size={14} />
          </button>
        ))}
      <MenuButton
        kind="icon"
        label={t("editor.toolbar.moreFormats")}
        icon={<EllipsisIcon size={14} />}
        menuLabel={t("editor.toolbar.moreFormats")}
        entries={moreEntries}
      />
    </div>
  );
}
