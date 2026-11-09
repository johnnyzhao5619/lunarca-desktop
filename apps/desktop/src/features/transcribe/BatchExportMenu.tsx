import { useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type TranscriptExportFormat } from "../../ipc/bindings";
import { IconButton } from "../../components/ui/IconButton";
import { DownloadIcon } from "@lunarca/editor-core/Icon";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu } from "../../lib/context-menu";
import { requestExportOptions } from "../../lib/app-events";
import { TRANSCRIPT_EXPORT_FORMATS, currentExportOptions, exportOptionsCustomized, useExportOptions } from "./export-options";


export interface BatchExportMenuProps {
  /** 待导出的转写笔记。 */
  noteRels: string[];
  /**
   * 这一批是**用户选中的**（而不是"当前这份结果的全部"）——按钮据此改写自己的说法。
   *
   * 按钮必须说清作用域：同一颗按钮在多选时导出 3 条、没多选时导出 40 条，写着同一句
   * 「导出全部」的话，用户按下去才知道刚才发生了什么。
   */
  selected?: boolean;
  onStatus: (message: string) => void;
  /**
   * 紧凑形态：只出图标（侧栏页头的计数行里，与「新建文件夹」并排）。
   * 默认仍是带文案的次级按钮（选中多条时列表底部那一枚）。
   */
  compact?: boolean;
}

/**
 * 批量导出菜单（PRD-05-021）。
 *
 * 服务字幕译者的真实闭环——导入 10 个文件后一次拿到 10 份 SRT，而不是逐篇点保存。
 * 选格式 → Rust 弹目录选择 → 每篇写 `<笔记名>.<扩展名>`；一篇无转写段不中断整批。
 * **此前是 `<details>` + `role="menu"` 自绘的一份**：声明了菜单角色，却没有 Esc、
 * 没有方向键、条目也不在 roving tabindex 里——鼠标点得开，键盘走不通。已改用全应用
 * 唯一的菜单实现（`useContextMenu`），那套行为一次写好，这里只提供条目。
 *
 * 内容取**原文**（批量场景无「当前显示模式」可跟随；需要双语请用单篇导出，
 * 避免在这里再塞一层二级选择把菜单变成迷宫）。
 */
export function BatchExportMenu({
  noteRels,
  selected = false,
  onStatus,
  compact = false,
}: BatchExportMenuProps) {
  const { t } = useTranslation("transcribe");
  const contextMenu = useContextMenu();
  const [exporting, setExporting] = useState(false);
  const disabled = noteRels.length === 0 || exporting;
  const customized = exportOptionsCustomized(useExportOptions());

  async function runExport(format: TranscriptExportFormat) {
    setExporting(true);
    const result = await commands.exportTranscripts(
      noteRels,
      format,
      "source",
      currentExportOptions(),
    );
    setExporting(false);
    if (result.status === "error") {
      onStatus(t("exportFailed", { error: result.error }));
      return;
    }
    // 0 = 用户取消目录选择（无需提示）。
    if (result.data > 0) {
      onStatus(t("exportDone", { count: result.data }));
    }
  }

  const label = exporting
    ? t("exporting")
    : selected
      ? t("exportSelected", { count: noteRels.length })
      : t("exportAll", { count: noteRels.length });

  function openMenu(event: React.MouseEvent) {
        contextMenu.open(
          event,
          buildMenu([
            ...TRANSCRIPT_EXPORT_FORMATS.map((format) => ({
              kind: "action" as const,
              id: format,
              label: t(`format.${format}`),
              onSelect: () => void runExport(format),
            })),
            // 时移 / 折行 / BOM / 说话人前缀（PRD-11-004/007）：应用级同一张对话框。
            {
              kind: "action" as const,
              id: "export-options",
              label: customized ? t("exportOptions.menuCustomized") : t("exportOptions.menu"),
              onSelect: () => requestExportOptions(),
            },
          ]),
        );
  }

  // 紧凑形态：侧栏页头计数行里的一枚图标键（与「新建文件夹」并排），可访问名即完整文案。
  if (compact) {
    return (
      <IconButton
        label={label}
        icon={<DownloadIcon size={14} />}
        disabled={disabled}
        aria-haspopup="menu"
        onClick={openMenu}
      />
    );
  }

  return (
    <button
      type="button"
      className="luna-btn luna-btn--secondary"
      disabled={disabled}
      aria-haspopup="menu"
      onClick={openMenu}
    >
      <DownloadIcon size={14} />
      <span>{label}</span>
    </button>
  );
}
