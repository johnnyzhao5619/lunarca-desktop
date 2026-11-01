/**
 * 字幕外观面板（ADR-137 H6 补，真机 2026-10-02：「各种显示方式应该在 HUD 上就能切，而不是
 * 还得进设置页」）。原文窗与译文窗共用这一份——两扇窗同一套外观设置，改了一起变。
 *
 * 只派发设置写入（`useCaptionAppearance.setAppearance`，与设置页同一个键），不在前端另存
 * 一份；声音来源也在这里（真机 2026-10-02：「声音来源应该能快速设，而不是进设置页」）。
 * 不常用的项（同屏行数）仍在设置页，面板底部给一条深链。
 */

import { useTranslation } from "react-i18next";
import { goTo } from "../../lib/navigate";
import type {
  CaptionAppearanceControls,
  CaptionFontSize,
  CaptionTextColor,
} from "../recording/use-caption-appearance";

const FONT_SIZES: readonly CaptionFontSize[] = ["small", "medium", "large", "xlarge"];
const TEXT_COLORS: readonly CaptionTextColor[] = ["white", "yellow", "cyan", "green"];

/** 双语排法：三种互斥的排法收成一组（真机 2026-10-02：「译文在上」与分窗分成两个开关时，
 *  分窗开着它就灰掉、且不说为什么，用户以为它坏了）。选任何一种都意味着「看双语」。 */
export type CaptionLayout = "source_first" | "translation_first" | "separate";

const LAYOUTS: readonly CaptionLayout[] = ["source_first", "translation_first", "separate"];
const CAPTURE_SOURCES = ["input_device", "system_loopback", "both"] as const;

interface CaptionAppearancePanelProps {
  appearance: CaptionAppearanceControls;
  /** 原文窗才有：声音来源与双语排法（译文窗本身就是分出去的那一扇）。 */
  session?: {
    layout: CaptionLayout | null;
    onLayoutChange: (layout: CaptionLayout) => void;
    captureSource: string;
    onCaptureSourceChange: (source: string) => void;
    /** 会话进行中：音频流已按开始时的来源打开，中途改不了——锁住并说出原因（不留一个
     *  「看着改了其实没变」的控件，界面上的双路标签也会跟着说谎）。 */
    sessionOpen: boolean;
  };
}

export function CaptionAppearancePanel({ appearance, session }: CaptionAppearancePanelProps) {
  const { t } = useTranslation("caption");
  const { t: tSettings } = useTranslation("settings");
  return (
    <div className="luna-caption__appearance" role="group" aria-label={t("appearance.label")} data-no-drag>
      {session ? (
        <>
          <div className="luna-caption__appearance-row">
            <span className="luna-caption__appearance-label">{tSettings("caption.capture_source")}</span>
            <div className="luna-caption__segmented">
              {CAPTURE_SOURCES.map((source) => (
                <button
                  key={source}
                  type="button"
                  className="luna-caption__segment"
                  aria-pressed={session.captureSource === source}
                  disabled={session.sessionOpen}
                  onClick={() => session.onCaptureSourceChange(source)}
                >
                  {tSettings(`option.caption.capture_source.${source}`)}
                </button>
              ))}
            </div>
          </div>
          {session.sessionOpen ? (
            <p className="luna-caption__appearance-note">{t("source.nextSession")}</p>
          ) : null}
          <div className="luna-caption__appearance-row">
            <span className="luna-caption__appearance-label">{t("layout.label")}</span>
            <div className="luna-caption__segmented">
              {LAYOUTS.map((layout) => (
                <button
                  key={layout}
                  type="button"
                  className="luna-caption__segment"
                  aria-pressed={session.layout === layout}
                  onClick={() => session.onLayoutChange(layout)}
                >
                  {t(`layout.${layout}`)}
                </button>
              ))}
            </div>
          </div>
        </>
      ) : null}
      <div className="luna-caption__appearance-row">
        <span className="luna-caption__appearance-label">{tSettings("caption.font_size")}</span>
        <div className="luna-caption__segmented">
          {FONT_SIZES.map((size) => (
            <button
              key={size}
              type="button"
              className="luna-caption__segment"
              aria-pressed={appearance.fontSize === size}
              onClick={() => appearance.setAppearance({ key: "caption.font_size", value: size })}
            >
              {tSettings(`option.caption.font_size.${size}`)}
            </button>
          ))}
        </div>
      </div>
      <div className="luna-caption__appearance-row">
        <span className="luna-caption__appearance-label">{tSettings("caption.text_color")}</span>
        <div className="luna-caption__swatches">
          {TEXT_COLORS.map((color) => (
            <button
              key={color}
              type="button"
              className="luna-caption__swatch"
              data-swatch={color}
              aria-pressed={appearance.textColor === color}
              aria-label={tSettings(`option.caption.text_color.${color}`)}
              title={tSettings(`option.caption.text_color.${color}`)}
              onClick={() => appearance.setAppearance({ key: "caption.text_color", value: color })}
            />
          ))}
        </div>
      </div>
      <label className="luna-caption__appearance-row">
        <span className="luna-caption__appearance-label">{t("appearance.background")}</span>
        <input
          type="range"
          className="luna-range luna-caption__appearance-slider"
          min={0}
          max={100}
          step={5}
          value={appearance.frameOpacity}
          onChange={(event) =>
            appearance.setAppearance({ key: "caption.frame_opacity", value: Number(event.currentTarget.value) })
          }
        />
      </label>
      <label className="luna-caption__appearance-row">
        <span className="luna-caption__appearance-label">{tSettings("caption.text_opacity")}</span>
        <input
          type="range"
          className="luna-range luna-caption__appearance-slider"
          min={0}
          max={100}
          step={5}
          value={appearance.textOpacity}
          onChange={(event) =>
            appearance.setAppearance({ key: "caption.text_opacity", value: Number(event.currentTarget.value) })
          }
        />
      </label>
      <button
        type="button"
        className="luna-caption__appearance-more"
        onClick={() => goTo("settings://translate")}
      >
        {t("appearance.moreSettings")}
      </button>
    </div>
  );
}
