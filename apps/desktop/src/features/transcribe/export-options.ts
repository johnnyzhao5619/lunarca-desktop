/**
 * 转写导出的**会话级选项**（PRD-11-004/007）：时移、BOM、折行覆盖、说话人前缀。
 *
 * 三个导出入口（回放导出条、批量导出菜单、清单右键菜单）读的都是这一份；「导出选项…」
 * 对话框改的也是这一份。它是纯呈现层的**选择记忆**，不是业务规则——每个值的语义与
 * 生效方式全在 Rust（`TranscriptExportOptionsDto` → `luna_export::ExportOptions`）。
 * BOM 的默认值来自设置 `export.utf8_bom`（对话框里改它即写回设置）；其余项进程内记住。
 */

import { useSyncExternalStore } from "react";
import type { HanVariantDto, TranscriptExportFormat, TranscriptExportOptionsDto } from "../../ipc/bindings";

/**
 * 转写的导出格式，按菜单顺序（PRD-11-001 / 014）。回放导出条、批量导出菜单、清单右键菜单
 * 共用这一份；显示名是 `transcribe:format.<值>`。
 */
export const TRANSCRIPT_EXPORT_FORMATS: readonly TranscriptExportFormat[] = [
  "srt",
  "vtt",
  "ass",
  "lrc",
  "bcc",
  "timestamped_txt",
  "plain_txt",
];

/**
 * 中文字形的可选项（PRD-11-013），按界面上的排列顺序。导出选项对话框与发布包页共用这一份，
 * 文案键是 `transcribe:exportOptions.han_<值>`。转换本身在 Rust（`luna_export::HanVariant`）。
 */
export const HAN_VARIANTS: readonly HanVariantDto[] = [
  "keep",
  "simplified",
  "traditional_taiwan",
  "traditional_hong_kong",
];

export const DEFAULT_EXPORT_OPTIONS: TranscriptExportOptionsDto = {
  time_offset_ms: 0,
  utf8_bom: false,
  max_line_len: null,
  max_lines: null,
  speaker_prefix: true,
  han_variant: "keep",
};

let current: TranscriptExportOptionsDto = DEFAULT_EXPORT_OPTIONS;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) {
    listener();
  }
}

/** 当前导出选项（导出命令发出前取一次）。 */
export function currentExportOptions(): TranscriptExportOptionsDto {
  return current;
}

/** 合并更新（对话框逐项改）。 */
export function updateExportOptions(patch: Partial<TranscriptExportOptionsDto>): void {
  current = { ...current, ...patch };
  emit();
}

/** 订阅（React 用）。 */
export function useExportOptions(): TranscriptExportOptionsDto {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    () => current,
    () => current,
  );
}

/** 有没有偏离默认（入口按钮据此显示「已调整」徽标）。 */
export function exportOptionsCustomized(options: TranscriptExportOptionsDto): boolean {
  return (
    options.time_offset_ms !== 0 ||
    options.max_line_len !== null ||
    options.max_lines !== null ||
    !options.speaker_prefix ||
    options.han_variant !== "keep"
  );
}
