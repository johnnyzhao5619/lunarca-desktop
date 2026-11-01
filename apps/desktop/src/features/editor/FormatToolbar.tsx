// 格式快捷编辑栏（PRD-06-014，Typora/Apple Notes 对标）：帮助不熟悉 Markdown 的
// 用户可视化施加格式——标题下拉 + 行内格式（激活态高亮）+ 列表/引用 + 链接 +
// 插入（表格/代码块/分隔线）。条目与动作复用 editor-actions（与右键菜单同一
// 来源，DRY）；仅编辑型模式渲染（阅读模式由 VaultEditor 决定不挂载）。
//
// 快捷键提示一律经 `formatItemShortcut` 从命令注册表派生——此前这里写死 `⌘B` / `⌘K` /
// `⌘⌥T` 三个字面量，是 keymap 之外的第三份键位事实源，且在 Windows 上显示的符号就是错的。

import { useTranslation } from "react-i18next";
import { useContextMenu } from "../../components/ContextMenuHost";
import { IconButton } from "../../components/ui/IconButton";
import { buildMenu, separator } from "../../lib/context-menu";
import {
  BoldIcon,
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
} from "../../components/ui/Icon";
import { BLOCK_ICONS, INLINE_ICONS } from "./format-icons";
import {
  formatItemShortcut,
  headingItems,
  inlineFormatItems,
  lineBlockItems,
  type EditorFormatAction,
} from "./editor-actions";
import type { EditorTier } from "./editor-tier";
import type { FormatState } from "./format-commands";

interface FormatToolbarProps {
  formatState: FormatState;
  onAction: (action: EditorFormatAction) => void;
  /**
   * 编辑器宽度档（`editor-tier.ts`，与页头同一判定）：非宽档时「图片 / 附件」也
   * 收进尾部的「更多格式」菜单——高频的段落 / 行内 / 块级按钮留在面上，低频的插入类先让位。
   */
  tier: EditorTier;
}

/** 「插入」组：无勾选态的一次性动作（图片/附件 / 表格 / 代码块 / 标注块 / 分隔线）。 */
const INSERT_ITEMS: readonly { key: string; labelKey: string; action: EditorFormatAction; Icon: typeof BoldIcon }[] = [
  { key: "attachment", labelKey: "menu.insertAttachment", action: { kind: "attachment" }, Icon: ImageIcon },
  { key: "table", labelKey: "table.insert", action: { kind: "table" }, Icon: TableIcon },
  { key: "codeBlock", labelKey: "menu.codeBlock", action: { kind: "codeBlock" }, Icon: CodeSquareIcon },
  { key: "callout", labelKey: "menu.callout", action: { kind: "callout" }, Icon: CalloutIcon },
  { key: "hr", labelKey: "menu.hr", action: { kind: "hr" }, Icon: MinusIcon },
];

/** 留在面上的插入类：宽档两格（图片 / 附件、表格），窄档只留表格。 */
const DIRECT_INSERT_KEYS: readonly string[] = ["attachment", "table"];

export function FormatToolbar({ formatState, onAction, tier }: FormatToolbarProps) {
  const { t } = useTranslation("editor");
  const contextMenu = useContextMenu();

  const run = (action: EditorFormatAction) => {
    onAction(action);
  };

  const headings = headingItems(formatState);
  const currentHeading = headings.find((item) => item.checked && item.key !== "text") ?? null;
  const headingLabel = currentHeading ? t(currentHeading.labelKey) : t("menu.paragraphText");

  /** 工具提示 = 名称（快捷键）；没绑键就只有名称。 */
  const title = (labelKey: string, itemKey: string) => {
    const shortcut = formatItemShortcut(itemKey);
    return shortcut ? t("toolbar.labelShortcut", { label: t(labelKey), shortcut }) : t(labelKey);
  };

  return (
    <div className="luna-format-toolbar" role="toolbar" aria-label={t("toolbar.label")}>
      {/* 标题级别此前是第四份自绘菜单：`role="menu"` + `menuitemcheckbox`，却没有
          Esc、没有方向键。已收编到全应用唯一的菜单实现——勾选态那套呈现它本来就有。 */}
      <button
        type="button"
        className="luna-format-toolbar__heading-trigger"
        aria-haspopup="menu"
        title={t("toolbar.headingTitle")}
        onClick={(event) => {
          contextMenu.open(
            event,
            buildMenu(
              headings.map((item) => ({
                kind: "action" as const,
                id: item.key,
                label: t(item.labelKey),
                checked: item.checked,
                shortcut: formatItemShortcut(item.key),
                onSelect: () => run(item.action),
              })),
            ),
          );
        }}
      >
        <HeadingIcon size={13} />
        <span className="luna-format-toolbar__heading-label">{headingLabel}</span>
        <ChevronDownIcon size={11} />
      </button>
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
            onClick={() => run(item.action)}
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
            onClick={() => run(item.action)}
          >
            <Icon size={14} />
          </button>
        );
      })}
      <span className="luna-format-toolbar__divider" aria-hidden="true" />
      <button
        type="button"
        className="luna-format-toolbar__btn"
        title={title("menu.insertLink", "link")}
        aria-label={t("menu.insertLink")}
        onClick={() => run({ kind: "link" })}
      >
        <LinkIcon size={14} />
      </button>
      {/* 直接动作（R5，2026-10-08）：宽档多给「图片 / 附件」一格，表格恒在面上；其余插入类
          （wiki 链接、代码块、标注块、分隔线）在**所有宽度**都收进同一个「更多格式」菜单——
          按工具组固定分组，不量字宽猜重要性；快捷键照旧可达。 */}
      {DIRECT_INSERT_KEYS.filter((key) => tier === "wide" || key === "table").map((key) => {
        const item = INSERT_ITEMS.find((entry) => entry.key === key)!;
        const { Icon } = item;
        return (
          <button
            key={key}
            type="button"
            className="luna-format-toolbar__btn"
            title={title(item.labelKey, key)}
            aria-label={t(item.labelKey)}
            onClick={() => run(item.action)}
          >
            <Icon size={14} />
          </button>
        );
      })}
      <IconButton
        density="sm"
        label={t("more.formats")}
        icon={<EllipsisIcon size={14} />}
        aria-haspopup="menu"
        onClick={(event) => {
          contextMenu.open(
            event,
            buildMenu([
              {
                kind: "action" as const,
                id: "wikiLink",
                label: t("menu.insertWikiLink"),
                icon: <WikiLinkIcon size={13} />,
                onSelect: () => run({ kind: "wikiLink" }),
              },
              separator("insert"),
              ...INSERT_ITEMS.filter(
                (item) => !DIRECT_INSERT_KEYS.includes(item.key) || (tier !== "wide" && item.key !== "table"),
              ).map(({ key, labelKey, action, Icon }) => ({
                kind: "action" as const,
                id: key,
                label: t(labelKey),
                icon: <Icon size={13} />,
                shortcut: formatItemShortcut(key),
                onSelect: () => run(action),
              })),
            ]),
            { label: t("more.formats") },
          );
        }}
      />
    </div>
  );
}
