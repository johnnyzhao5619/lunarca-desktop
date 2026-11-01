/**
 * 实时字幕视图（录音 HUD 与实时字幕面板共用，DRY）。
 *
 * 影片字幕/歌词式呈现：每条 cue 原文一行、译文一行（可单/双语），暂定尾部灰显。
 * 自动贴底滚动，只呈现最近数条。样式由 `data-variant` 控制字号密度（hud 紧凑 / caption 大字）。
 */

import { useEffect, useRef } from "react";
import type { SubtitleView } from "./live-subtitle";

interface LiveSubtitleViewProps {
  view: SubtitleView;
  /** 是否显示原文（转写关时可仅显示译文；均关则显示空态）。 */
  showSource: boolean;
  /** 是否显示译文行（转译关闭则不显示）。 */
  showTranslation: boolean;
  /** 双语的视觉与辅助技术阅读顺序一致。 */
  translationFirst?: boolean;
  /** 字号密度：hud = 紧凑（面板内）；caption = 大字（独立字幕窗）。 */
  variant: "hud" | "caption";
  /** 无内容时的占位提示。 */
  emptyLabel: string;
  /**
   * 是否进入读屏 live 区（默认是）。录音 HUD 传 `false`：那扇窗的状态一句另有 live 区，
   * 字幕每来一段都播报会持续打断朗读；独立字幕窗字幕就是主体，保持播报。
   */
  announce?: boolean;
  /** 原文行 aria-label（屏幕阅读器）。 */
  sourceAriaLabel: string;
  /** 轨级标签文案（PRD-04-053 L0：self=我 / remote=对方）；缺省不渲染轨标签。 */
  trackLabels?: { self: string; remote: string };
}

export function LiveSubtitleView({
  view,
  showSource,
  showTranslation,
  translationFirst = false,
  variant,
  announce = true,
  emptyLabel,
  sourceAriaLabel,
  trackLabels,
}: LiveSubtitleViewProps) {
  const trackLabel = (track: string | null | undefined): string | null => {
    if (!trackLabels || !track) return null;
    if (track === "self") return trackLabels.self;
    if (track === "remote") return trackLabels.remote;
    return null;
  };
  const scrollRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [view]);

  const hasContent = view.cues.length > 0 || view.hypotheses.length > 0;

  return (
    <div
      ref={scrollRef}
      className="luna-subtitle"
      data-variant={variant}
      role="log"
      aria-live={announce ? "polite" : "off"}
      aria-label={sourceAriaLabel}
    >
      {hasContent ? (
        <>
          {view.cues.map((cue) => {
            const sourceLine = showSource ? (
              <p className="luna-subtitle__source">
                {trackLabel(cue.track) && (
                  <span className="luna-subtitle__track" data-track={cue.track ?? undefined}>
                    {trackLabel(cue.track)}
                  </span>
                )}
                {cue.source}
              </p>
            ) : null;
            const translationLine = showTranslation && cue.translation ? (
              <p className="luna-subtitle__translation">{cue.translation}</p>
            ) : null;
            return (
              <div key={`${cue.startMs}-${cue.source}`} className="luna-subtitle__cue">
                {translationFirst ? <>{translationLine}{sourceLine}</> : <>{sourceLine}{translationLine}</>}
              </div>
            );
          })}
          {showSource &&
            view.hypotheses.map((hypothesis) => (
              <p key={hypothesis.track ?? "primary"} className="luna-subtitle__hypothesis">
                {trackLabel(hypothesis.track) && (
                  <span className="luna-subtitle__track" data-track={hypothesis.track ?? undefined}>
                    {trackLabel(hypothesis.track)}
                  </span>
                )}
                {hypothesis.text}
              </p>
            ))}
        </>
      ) : (
        <p className="luna-subtitle__empty">{emptyLabel}</p>
      )}
    </div>
  );
}
