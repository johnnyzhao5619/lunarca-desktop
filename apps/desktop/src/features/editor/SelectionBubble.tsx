// 选区气泡条（PRD-06-020 交互重构，Notion 浮动工具栏 / Google Docs Refine 对标）：
// 选区稳定后只出现这一条细气泡——格式按钮 + ✦ AI + 朗读所选 + 相关笔记。AI 面板不再一选
// 即弹，由 ✦ 或 ⌘J 主动召唤；于是读文、拖选、复制不会被一扇 400px 的面板遮住下一行。
//
// 定位由气泡**自己**量自己的尺寸（`useBoxSize`）再算：优先在选区上方居中，放不下落到下方。
// 宿主只给选区包围盒与编辑器尺寸这两个事实。

import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { cssVariables } from "../../lib/css-variables";
import { useBoxSize } from "../../hooks/use-resize-observer";
import { QuoteIcon, SearchIcon, SparklesIcon, SpeakIcon } from "@lunarca/editor-core/Icon";
import type { EditorFormatAction } from "@lunarca/editor-core/editor-actions";
import type { FormatState } from "@lunarca/editor-core/format-commands";
import { placeSelectionBubble, type EditorBox, type SelectionBox } from "./floating-menu-position";
import { SelectionFormatBar } from "./SelectionFormatBar";

/** 首帧（尚未量到自身尺寸）的占位尺寸；layout effect 量到真实值后同帧重算。 */
const FALLBACK_SIZE = { width: 320, height: 36 };

interface SelectionBubbleProps {
  box: SelectionBox;
  editorSize: EditorBox;
  formatState: FormatState;
  onFormatAction: (action: EditorFormatAction) => void;
  aiTitle: string;
  onAi: () => void;
  /** `null` = 朗读不可用（不渲染该钮）。 */
  onRead: (() => void) | null;
  onRelated: () => void;
  /** 「存为引文」（ADR-162 ⑭）；`null` = 这类笔记不给（脚本、项目主页）。 */
  onQuote: (() => void) | null;
}

export function SelectionBubble({
  box,
  editorSize,
  formatState,
  onFormatAction,
  aiTitle,
  onAi,
  onRead,
  onRelated,
  onQuote,
}: SelectionBubbleProps) {
  const { t } = useTranslation("editor");
  const ref = useRef<HTMLDivElement | null>(null);
  const size = useBoxSize(ref);
  const placed = placeSelectionBubble(
    box,
    editorSize,
    size ? { width: size.inline, height: size.block } : FALLBACK_SIZE,
  );

  return (
    <div
      ref={ref}
      className="luna-selection-bubble"
      role="toolbar"
      aria-label={t("floating.title")}
      data-placement={placed.placement}
      // 动态坐标经 CSS 变量下发（与 AI 面板同一先例），样式仍在 editor-core.css。
      style={cssVariables({
        "--selection-bubble-x": `${placed.x}px`,
        "--selection-bubble-y": `${placed.y}px`,
      })}
      // 气泡上的点击不能把编辑器选区弄丢（mousedown 会把焦点移走并清空选区）。
      onMouseDown={(event) => {
        event.preventDefault();
      }}
    >
      <SelectionFormatBar formatState={formatState} onAction={onFormatAction} />
      <span className="luna-format-toolbar__divider" aria-hidden="true" />
      <button
        type="button"
        className="luna-selection-bubble__ai"
        title={aiTitle}
        aria-label={aiTitle}
        onClick={onAi}
      >
        <SparklesIcon size={14} />
        {t("floating.bubble.ai")}
      </button>
      {onRead ? (
        <button
          type="button"
          className="luna-format-toolbar__btn"
          title={t("read.selection")}
          aria-label={t("read.selection")}
          onClick={onRead}
        >
          <SpeakIcon size={14} />
        </button>
      ) : null}
      <button
        type="button"
        className="luna-format-toolbar__btn"
        title={t("floating.bubble.related")}
        aria-label={t("floating.bubble.related")}
        onClick={onRelated}
      >
        <SearchIcon size={14} />
      </button>
      {onQuote ? (
        <button
          type="button"
          className="luna-format-toolbar__btn"
          title={t("floating.bubble.quote")}
          aria-label={t("floating.bubble.quote")}
          onClick={onQuote}
        >
          <QuoteIcon size={14} />
        </button>
      ) : null}
    </div>
  );
}
