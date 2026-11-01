import { useTranslation } from "react-i18next";
import { type PresetInfo } from "../../ipc/bindings";
import { FailureCard } from "../../components/ui/FailureCard";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon, StopCircleIcon } from "../../components/ui/Icon";
import { TaskStatus, type TaskStatusKind } from "../../components/TaskStatus";
import type { PopupSlot, PopupSlotStatus } from "./popup-slots";
import { metaLabel } from "./popup-meta-label";
import { PresetSelect } from "./PresetSelect";
import { ResultActionBar } from "./ResultActionBar";
import { goTo } from "../../lib/navigate";
import { usePanelTextEditing } from "../../hooks/use-panel-text-editing";

type Translate = (key: string, options?: Record<string, unknown>) => string;

type PopupResultSlotProps = {
  presets: PresetInfo[];
  slot: PopupSlot;
  copied?: boolean;
  onCopy: () => void;
  onPresetChange: (presetId: string) => void;
  onRegenerate: () => void;
  /**
   * 停止这一槽的生成（保留已产 token，后端回 `cancelled` 终态）。不传 = 这一槽的停止由
   * 别处承载（输入模式下输入行主按钮生成中即为「停止」，同一件事不放两枚按钮）。
   */
  onStop?: () => void;
  onOutputChange: (output: string) => void;
  onReplace: () => void;
  onSaveNote: () => void;
  showReplace: boolean;
  /** 移除这一槽（对比槽才有；此前对比槽只能加不能减，只能靠重开窗复位）。 */
  onRemove?: () => void;
};

export function popupSlotTaskStatusKind(status: PopupSlotStatus): TaskStatusKind {
  switch (status) {
    case "running":
      return "processing";
    case "done":
    case "cache_hit":
      return "completed";
    case "failed":
    case "error":
      return "failed";
    case "cancelled":
      return "cancelled";
    case "idle":
      return "idle";
  }
}

/// 状态行只在**传递有效信息**时出现：进行中/缓存命中/取消/动作回执。
/// `idle`/`done` 不渲染独立状态行——结果本身即完成信号，省一行高度（信息密度优先）；
/// 失败/错误不走此状态胶囊，由专用错误卡（可换行 + 修复动作）呈现。
function slotStatusLabel(slot: PopupSlot, t: Translate): string | null {
  if (slot.status === "cache_hit") {
    return t("status.cache_hit");
  }
  if (slot.status === "cancelled") {
    return t("status.cancelled");
  }
  if (slot.status === "running") {
    return t("status.running");
  }
  // done/idle：动作回执（已复制/已存入笔记等）仍要展示。
  return slot.message;
}

export function PopupResultSlot({
  presets,
  slot,
  copied = false,
  onCopy,
  onPresetChange,
  onRegenerate,
  onStop,
  onOutputChange,
  onReplace,
  onSaveNote,
  showReplace,
  onRemove,
}: PopupResultSlotProps) {
  const { t } = useTranslation("popup");
  const { t: tCommon } = useTranslation("common");
  // 译文框可编辑：编辑期上报，让面板取得键盘（Windows 上展示模式的弹窗不可激活，不上报的话
  // 键入会落进源应用，ADR-163 ①）。
  const outputEditing = usePanelTextEditing();
  const canUseResult = slot.output.trim().length > 0;
  const isFailure = slot.status === "failed" || slot.status === "error";
  const statusLabel = slotStatusLabel(slot, t);
  const taskStatusLabel = statusLabel ?? t(`status.${slot.status}`);
  const toolbar = (
    <div className="luna-popup-slot__toolbar">
      <span className="luna-popup-slot__toolbar-label">
        {slot.meta ? metaLabel(slot.meta, presets, t, tCommon) : t("slot.title")}
      </span>
      <PresetSelect presets={presets} value={slot.presetId} onChange={onPresetChange} />
      {onRemove ? (
        <IconButton
          density="hud"
          label={t("action.remove_compare")}
          icon={<CloseIcon size={14} />}
          onClick={onRemove}
        />
      ) : null}
    </div>
  );
  // 失败即塌缩（ADR-111 ⑤⑶）：只剩失败卡（成因 + 修复深链 + 重试）——不陈列空的
  // 可编辑输出区，也不陈列灰着的复制/替换动作条。工具行保留：换个目标语言/预设
  // 重译本身就是一种修复。中途失败的半截译文一并舍弃：对半句话的操作没有一个是
  // 用户想要的，重译才是。消息用事件里已本地化好的那句（含码驱动的修复文案）。
  if (isFailure) {
    return (
      <section className="luna-popup-slot" aria-label={t("slot.title")}>
        {toolbar}
        <FailureCard
          code={slot.failureCode}
          message={slot.message ?? t(`status.${slot.status}`)}
          retry={{ label: t("action.regenerate"), onClick: onRegenerate }}
        />
      </section>
    );
  }
  return (
    <section className="luna-popup-slot" aria-label={t("slot.title")}>
      {toolbar}
      {statusLabel ? (
        <div className="luna-popup-slot__status">
          <TaskStatus kind={popupSlotTaskStatusKind(slot.status)} label={taskStatusLabel} />
          {slot.status === "running" && onStop ? (
            // 生成中随时可停（对标 DeepL / Bob）：此前弹窗没有任何取消通路，关窗后旧 Job
            // 仍跑完并计费。带文字的次级按钮，与笔记 AI 面板的「停止」同一形态——
            // 真机反馈：只有一枚图标时用户根本没认出它是停止。
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-popup-slot__stop"
              onClick={onStop}
            >
              <StopCircleIcon size={14} />
              <span>{t("action.stop")}</span>
            </button>
          ) : null}
          {slot.savedPath ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => goTo(`notes://${slot.savedPath}`)}
            >
              {t("action.open_saved_note")}
            </button>
          ) : null}
        </div>
      ) : null}
      {/* 译文框按内容高（ADR-137 V7，镜像格子同捕获框）：一个词不占五行，长段落到上限才在框内滚动。 */}
      <div className="luna-popup__output-grow" data-value={slot.output}>
        <textarea
          className="luna-popup__output"
          aria-label={t("slot.editOutput")}
          value={slot.output}
          readOnly={slot.status === "running"}
          onFocus={outputEditing.onFocus}
          onPointerDown={outputEditing.onPointerDown}
          onBlur={outputEditing.onBlur}
          onChange={(event) => onOutputChange(event.currentTarget.value)}
        />
      </div>
      {slot.status !== "idle" ? (
        <ResultActionBar
          canUseResult={canUseResult}
          canSaveNote={slot.savedPath === null}
          copied={copied}
          onCopy={onCopy}
          onRegenerate={onRegenerate}
          onReplace={onReplace}
          onSaveNote={onSaveNote}
          showReplace={showReplace}
          speakText={slot.output}
          speakLang={slot.meta?.target ?? null}
          speakScope={`hud:translation:${slot.id}`}
        />
      ) : null}
    </section>
  );
}
