/**
 * 视频舞台（PRD-04-058 / PRD-05-023，Descript / Otter / IINA 对标）。
 *
 * 导入的视频在同步回放视图里**就地放画面**，并把转写/转译结果作为字幕叠在画面上：
 * - 画面呈现四选（设置 `playback.video_layout` 持久化，跨笔记记忆）：
 *   分屏（默认，转写在左画面在右）/ 剧场（画面占满内容区）/ 悬浮小窗（可拖动）/ 仅音频。
 *   四个入口**带文字标签**——纯图标的呈现方式切换没人认得出「仅音频 = 关掉画面」。
 * - 字幕叠加内容 = **当前 cue**，跟随回放已有的原文/译文/双语三态（不另立一套开关，DRY），
 *   字号档与实时字幕面板同源；可整体关闭（`playback.video_subtitles`）。
 * - 画面点击 = 播放/暂停、双击 = 剧场/分屏来回（播放器通用心智）——这两件事随画面元素
 *   一起归了常驻宿主的画面层。
 *
 * 元素契约（关键，2026-09-08 随 ADR-118 上移）：`<video>` **不在本组件里**——它与压在
 * 画面上的两件覆盖物（起播键 / 字幕）住在常驻宿主 `playback-host.tsx` 的画面层里，随工作区
 * 常驻、永不卸载。本组件只留**工具条**与一个被测量的**画面槽位**（`__frame`），并把槽位
 * 注册给宿主；宿主把画面层贴到槽位矩形上。
 * 这样三件事同时成立：换布局不重建元素（原契约）、**切走空间也不重建元素**（新）、
 * 悬浮小窗被拖动时画面自动跟随（槽位在哪画面在哪，宿主不需要知道任何布局细节）。
 *
 * **控制行与画面分家（2026-09-08 真机）**：呈现切换 / 字幕开关 / 字号档此前长在画面
 * 舞台里，于是随画面走——分屏在右栏顶、剧场在全宽顶、小窗缩在小窗里、仅音频掉到列表底，
 * 四种呈现四个位置。YouTube / IINA / Descript 的呈现控件都固定在一处，不随画面挪。现在
 * `VideoStageControls` 由回放视图渲染在走带区之下的**固定一行**，四种呈现下位置不变；
 * 本组件只剩画面槽位与悬浮态的拖拽把手。
 *
 * 悬浮小窗的拖拽契约（PRD-04-058 真机反馈）：拖拽把手是小窗顶部那一条**把手**，按在控件
 * 上一律不起拖，且**位移达阈值后才** `setPointerCapture` —— 早先在 `pointerdown` 即捕获，
 * 指针捕获把随后的 click 重定向到把手本身，于是小窗里的控件全部失效。
 */

import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { cssVariables } from "../../lib/css-variables";
import type { PlaybackMediaKind } from "../../ipc/bindings";
import {
  CaptionsIcon,
  ExpandIcon,
  GripIcon,
  MusicIcon,
  PictureInPictureIcon,
  SplitIcon,
} from "../../components/ui/Icon";
import { shouldStartDrag } from "../../lib/pointer-drag";
import {
  clampFloatPosition,
  defaultFloatPosition,
  SUBTITLE_SIZES,
  VIDEO_LAYOUTS,
  type FloatPosition,
  type SubtitleSize,
  type VideoLayout,
} from "./video-stage";
import { SelectBox } from "../../components/ui/SelectBox";

/** 悬浮小窗尚未测量到真实尺寸时的估值（仅用于首次定位，随后按实测重夹）。 */
const FLOAT_ESTIMATED_SIZE = { width: 360, height: 240 };

const LAYOUT_ICONS: Record<VideoLayout, React.ReactNode> = {
  split: <SplitIcon size={14} />,
  theater: <ExpandIcon size={14} />,
  float: <PictureInPictureIcon size={14} />,
  audio_only: <MusicIcon size={14} />,
};

export interface VideoStageControlsProps {
  layout: VideoLayout;
  onLayoutChange: (layout: VideoLayout) => void;
  subtitlesEnabled: boolean;
  onSubtitlesToggle: (enabled: boolean) => void;
  subtitleSize: SubtitleSize;
  onSubtitleSizeChange: (size: SubtitleSize) => void;
}

/**
 * 呈现控制行：布局四选 + 字幕开关 + 字号档（+ 仅音频态的一句说明）。
 * 由回放视图渲染在走带区之下，**不随画面走**；音频笔记时调用方不渲染它。
 */
export function VideoStageControls({
  layout,
  onLayoutChange,
  subtitlesEnabled,
  onSubtitlesToggle,
  subtitleSize,
  onSubtitleSizeChange,
}: VideoStageControlsProps) {
  const { t } = useTranslation("playback");
  return (
    <div className="luna-video-stage__toolbar" role="group" aria-label={t("video.toolbar")}>
      <span
        className="luna-video-stage__layouts"
        role="group"
        aria-label={t("video.layoutLabel")}
      >
        {VIDEO_LAYOUTS.map((mode) => (
          <button
            key={mode}
            type="button"
            className="luna-btn luna-btn--secondary luna-video-stage__btn"
            aria-pressed={layout === mode}
            title={t(`video.layout.${mode}`)}
            aria-label={t(`video.layout.${mode}`)}
            onClick={() => onLayoutChange(mode)}
          >
            {LAYOUT_ICONS[mode]}
            <span className="luna-video-stage__btn-label">{t(`video.layoutShort.${mode}`)}</span>
          </button>
        ))}
      </span>
      {/* 仅音频态：画面收起后给一句话说明现在是什么状态、怎么找回画面 */}
      {layout === "audio_only" && (
        <p className="luna-video-stage__hidden-hint">{t("video.hiddenHint")}</p>
      )}
      <span className="luna-video-stage__spacer" />
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-video-stage__btn"
        aria-pressed={subtitlesEnabled}
        title={t(subtitlesEnabled ? "video.subtitlesOff" : "video.subtitlesOn")}
        aria-label={t(subtitlesEnabled ? "video.subtitlesOff" : "video.subtitlesOn")}
        onClick={() => onSubtitlesToggle(!subtitlesEnabled)}
      >
        <CaptionsIcon size={14} />
      </button>
      <SelectBox
        size="sm"
        className="luna-video-stage__size"
        value={subtitleSize}
        disabled={!subtitlesEnabled}
        ariaLabel={t("video.subtitleSize")}
        title={t("video.subtitleSize")}
        onChange={(event) => onSubtitleSizeChange(event.currentTarget.value as SubtitleSize)}
      >
        {SUBTITLE_SIZES.map((size) => (
          <option key={size} value={size}>
            {t(`video.size.${size}`)}
          </option>
        ))}
      </SelectBox>
    </div>
  );
}

export interface VideoStageProps {
  /** 媒体类型：`audio` 时整个舞台隐藏（元素仍在宿主里出声）。 */
  mediaKind: PlaybackMediaKind;
  layout: VideoLayout;
  /**
   * 把画面槽位交给常驻宿主（ADR-118）；卸载时宿主收到 `null`。
   *
   * 首参是**这一个槽位实例**的身份：只有当前持有者能注销自己，否则分屏切布局时
   * 旧槽位迟到的卸载回调会把新槽位抹掉（见 `stage-registration.ts`）。
   */
  registerStage: (stageId: string, element: HTMLElement | null) => void;
}

export function VideoStage({ mediaKind, layout, registerStage }: VideoStageProps) {
  const { t } = useTranslation("playback");
  // 槽位身份：`useId()` 在这一个实例的整个生命周期内稳定，重挂载即换新身份——正是
  // 「谁能注销谁」需要的那个粒度。`attachStage` 的引用必须同样稳定，否则每次渲染
  // React 都会先用 `null` 再用元素调一遍 ref。
  const stageId = useId();
  const attachStage = useCallback(
    (element: HTMLElement | null) => registerStage(stageId, element),
    [registerStage, stageId],
  );
  const stageRef = useRef<HTMLElement | null>(null);
  /** 悬浮小窗位置（会话内有效；`null` = 尚未定位，取默认右下角）。 */
  const [floatPos, setFloatPos] = useState<FloatPosition | null>(null);
  /** 位置的最新值（供 effect 读取而不因位置变化重跑：拖动中不该重置停靠点）。 */
  const floatPosRef = useRef<FloatPosition | null>(null);
  floatPosRef.current = floatPos;
  const isVideo = mediaKind === "video";
  const floating = isVideo && layout === "float";

  /** 按元素实测尺寸把位置夹回视口（进入悬浮态、拖动收尾、窗口缩放共用）。 */
  const settlePosition = useCallback((next: FloatPosition | null) => {
    const rect = stageRef.current?.getBoundingClientRect();
    const size = rect?.width ? { width: rect.width, height: rect.height } : FLOAT_ESTIMATED_SIZE;
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    setFloatPos(
      next ? clampFloatPosition(next, size, viewport) : defaultFloatPosition(size, viewport),
    );
  }, []);

  // 首次进入悬浮态定位到右下角（之后**保留用户拖到的位置**，来回切换不重置）；
  // 窗口缩放后按实测尺寸重夹，否则缩小窗口会把小窗留在视口外。
  useEffect(() => {
    if (!floating) {
      return;
    }
    settlePosition(floatPosRef.current);
    const onResize = () => settlePosition(floatPosRef.current);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [floating, settlePosition]);

  /**
   * 拖动悬浮小窗：按在工具条空白处起拖，位移达阈值（与树内拖拽同一 `shouldStartDrag`）
   * 才真正开始并捕获指针——**不达阈值绝不捕获**，否则会把控件的 click 一并吞掉。
   */
  function startFloatDrag(event: React.PointerEvent<HTMLDivElement>) {
    if (!floating || event.button !== 0) {
      return;
    }
    // 按在控件上 = 用户要点它，不是要拖窗（小窗里布局/字幕按钮全靠这一条才点得动）。
    if ((event.target as HTMLElement).closest("button, select, input, a")) {
      return;
    }
    const stage = stageRef.current;
    const rect = stage?.getBoundingClientRect();
    if (!stage || !rect) {
      return;
    }
    const offsetX = event.clientX - rect.left;
    const offsetY = event.clientY - rect.top;
    const size = { width: rect.width, height: rect.height };
    const origin = { x: event.clientX, y: event.clientY };
    const pointerId = event.pointerId;
    let started = false;

    const onMove = (moveEvent: PointerEvent) => {
      if (!started) {
        if (!shouldStartDrag(moveEvent.clientX - origin.x, moveEvent.clientY - origin.y)) {
          return;
        }
        started = true;
        stage.setPointerCapture(pointerId);
      }
      setFloatPos(
        clampFloatPosition(
          { x: moveEvent.clientX - offsetX, y: moveEvent.clientY - offsetY },
          size,
          { width: window.innerWidth, height: window.innerHeight },
        ),
      );
    };
    const onEnd = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onEnd);
      window.removeEventListener("pointercancel", onEnd);
      if (started && stage.hasPointerCapture(pointerId)) {
        stage.releasePointerCapture(pointerId);
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onEnd);
    window.addEventListener("pointercancel", onEnd);
  }

  return (
    <section
      ref={stageRef}
      className="luna-video-stage"
      data-kind={mediaKind}
      data-layout={layout}
      data-float-pending={floating && floatPos === null}
      aria-label={isVideo ? t("video.title") : undefined}
      aria-hidden={isVideo ? undefined : true}
      style={
        floating && floatPos
          ? cssVariables({
              "--video-float-x": `${floatPos.x}px`,
              "--video-float-y": `${floatPos.y}px`,
            })
          : undefined
      }
    >
      {/* 悬浮态的拖拽把手：小窗唯一的自带控件。呈现切换等在走带区之下的固定一行。 */}
      {floating && (
        <div
          className="luna-video-stage__handle"
          onPointerDown={startFloatDrag}
          title={t("video.dragHint")}
        >
          <GripIcon size={14} className="luna-video-stage__grip" />
          <span className="luna-video-stage__handle-label">{t("video.title")}</span>
        </div>
      )}

      {/* 画面槽位：这里**刻意是空的**——常驻画面层贴在它的矩形上（ADR-118）。它仍然占位、
          仍然决定画面多大在哪，只是不再拥有元素；卸载时把 `null` 交回宿主。 */}
      <div ref={attachStage} className="luna-video-stage__frame" />
    </section>
  );
}
