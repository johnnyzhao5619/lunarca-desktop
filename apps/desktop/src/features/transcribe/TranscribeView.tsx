import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { FilmIcon, LinkIcon, SettingsIcon } from "@lunarca/editor-core/Icon";
import type { TranscribeImport } from "./use-transcribe-import";

export interface TranscribeViewProps {
  /** 当前选中的转写笔记；null = 未选中（渲染导入引导）。 */
  selectedPath: string | null;
  /** 库内是否已有转写笔记（区分「从未导入」与「有数据但未选中」，PRD-07-020/021）。 */
  hasNotes: boolean;
  /** 回放由应用组合层提供，避免转写功能反向依赖播放功能（队列任务由回放视图按篇自订）。 */
  renderPlayback: (noteRel: string) => ReactNode;
  importer: TranscribeImport;
  onImport: () => void;
  /** 唤起「从链接导入」对话框（PRD-05-006）；「从链接导入」应用没启用时不给（ADR-167 ⑤）。 */
  onImportUrl?: () => void;
  onOpenSettings: () => void;
}

/**
 * 「转写」主区（PRD-05-030）。
 *
 * 两态：选中一篇 → 同步回放视图（与录音产物**共用同一时间轴文本模型**，PRD-05-022，
 * 不为导入另写一套回放）；未选中 → 导入引导卡（首次为空给一句结果 + 两个来源，
 * 已有数据只给选择提示，PRD-07-020/021 严格区分）。
 *
 * 引导卡只回答"从哪开始"：校对、翻译、区分说话人、导出是文稿出现之后工具栏上的动作，
 * 在空库里预告它们只是第二遍教学（2026-10-08 文案与交互审查 F06）。支持的格式紧跟导入
 * 动作说一次——侧栏空态不再重复。
 */
export function TranscribeView({
  selectedPath,
  hasNotes,
  renderPlayback,
  importer,
  onImport,
  onImportUrl,
  onOpenSettings,
}: TranscribeViewProps) {
  const { t } = useTranslation("transcribe");

  if (selectedPath) {
    return renderPlayback(selectedPath);
  }

  if (hasNotes) {
    return (
      <div className="luna-workspace-placeholder">
        <p>{t("selectHint")}</p>
      </div>
    );
  }

  return (
    <section className="luna-transcribe-landing">
      <FilmIcon size={40} className="luna-transcribe-landing__icon" />
      <h1>{t("landingTitle")}</h1>
      <p className="luna-transcribe-landing__body">{t("landingBody")}</p>
      <div className="luna-transcribe-landing__actions">
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          disabled={importer.importing}
          onClick={onImport}
        >
          <FilmIcon size={14} />
          <span>{importer.importing ? t("importing") : t("import")}</span>
        </button>
        {onImportUrl ? (
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onImportUrl}>
            <LinkIcon size={14} />
            <span>{t("fetch.entry")}</span>
          </button>
        ) : null}
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onOpenSettings}>
          <SettingsIcon size={14} />
          <span>{t("openSettings")}</span>
        </button>
      </div>
      <p className="luna-transcribe-landing__hint">{t("formatsHint")}</p>
    </section>
  );
}
