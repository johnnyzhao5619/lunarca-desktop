/**
 * 「正在播放」条（PRD-04-064）。
 *
 * 用户离开回放视图后，声音由常驻宿主继续放（ADR-118），这条是它**唯一的可见证据与
 * 操作面**——没有它，后台播放就成了"声音从哪儿来的不知道、想停也找不到地方"。
 *
 * 形态对标 Spotify / Apple Podcasts 的底部常驻条，控件只收最高频的四件：播放暂停、
 * ±10 秒、倍速、回到回放视图；外加一个关闭（= 卸载媒体，声音随之停）。当前句原文
 * 单行显示，让人一眼确认"现在放到哪儿了"。
 *
 * **它不重建任何播放状态**：全部取自宿主上下文，`<video>` 仍是那一个。
 */

import { useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { cssVariables } from "../../lib/css-variables";
import { CloseIcon, PauseIcon, PlayIcon } from "@lunarca/editor-core/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { formatClock } from "../../lib/format-duration";
import { noteStem } from "../../lib/note-path";
import { usePlaybackHost } from "./playback-host";
import { PLAYBACK_RATES, type PlaybackRate } from "./use-playback";
import { goTo } from "../../lib/navigate";

/** 走带 ±跳转步长（与回放视图同一档，PRD-04 §3.4）。 */
const SKIP_MS = 10_000;

export function NowPlayingBar() {
  const { t } = useTranslation("playback");
  const {
    noteRel,
    playbackState,
    actions,
    close,
    reopenHref,
    editorShowsPlayingNote,
    followInEditor,
    setFollowInEditor,
    videoPrefs,
  } = usePlaybackHost();
  const {
    playing,
    positionMs,
    durationMs,
    playbackRate,
    cues,
    activeCueIndex,
  } = playbackState;

  const cueRef = useRef<HTMLSpanElement | null>(null);
  /** 这一句放不下多少（px）；`0` = 放得下，不滚。 */
  const [cueOverflow, setCueOverflow] = useState(0);

  const activeCue = activeCueIndex >= 0 ? cues[activeCueIndex] : undefined;
  const cueText = activeCue?.text ?? "";

  // 溢出量只能实测：纯 CSS 问不出"这行字比格子宽多少"，而滚动距离必须正好是这个差。
  // 换句、改窗宽都要重量一次；量不出溢出就不加 `data-overflow`，静止的一行不该有动画。
  useLayoutEffect(() => {
    const measure = () => {
      const element = cueRef.current;
      if (!element) return;
      setCueOverflow(Math.max(0, element.scrollWidth - element.clientWidth));
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [cueText]);

  if (noteRel === null) return null;

  const title = noteStem(noteRel);

  return (
    <div
      className="luna-now-playing"
      role="region"
      aria-label={t("nowPlaying.label", { title })}
    >
      <div
        className="luna-now-playing__transport"
        role="group"
        aria-label={t("aria.controls")}
      >
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-now-playing__btn"
          disabled={durationMs <= 0}
          aria-label={t("action.back10")}
          title={t("action.back10")}
          onClick={() => actions.seekToMs(Math.max(0, positionMs - SKIP_MS))}
        >
          {t("action.back10Short")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-now-playing__btn"
          aria-label={playing ? t("action.pause") : t("action.play")}
          onClick={() => actions.togglePlay()}
        >
          {/* 非颜色-only：播放/暂停靠**形状**区分（同回放视图的取舍）。 */}
          {playing ? <PauseIcon size={14} /> : <PlayIcon size={14} />}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-now-playing__btn"
          disabled={durationMs <= 0}
          aria-label={t("action.forward10")}
          title={t("action.forward10")}
          onClick={() =>
            actions.seekToMs(Math.min(durationMs, positionMs + SKIP_MS))
          }
        >
          {t("action.forward10Short")}
        </button>
      </div>

      <div className="luna-now-playing__meta">
        <span className="luna-now-playing__title" title={title}>
          {title}
        </span>
        {/* 当前句：一眼确认放到哪儿了。空隙/起播前为空行，不占跳动的高度。 */}
        <span
          ref={cueRef}
          className="luna-now-playing__cue"
          data-overflow={cueOverflow > 0}
          title={cueText || undefined}
          style={cssVariables({ "--cue-overflow": `${cueOverflow}px` })}
        >
          {cueText}
        </span>
      </div>

      <span className="luna-now-playing__clock">
        {formatClock(positionMs)} / {formatClock(durationMs)}
      </span>

      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm luna-now-playing__rate"
        aria-label={t("speed.cycle", { rate: playbackRate })}
        title={t("speed.cycle", { rate: playbackRate })}
        onClick={() => {
          const next = PLAYBACK_RATES[
            (PLAYBACK_RATES.indexOf(playbackRate) + 1) % PLAYBACK_RATES.length
          ] as PlaybackRate;
          actions.setPlaybackRate(next);
        }}
      >
        {t("speed.option", { rate: playbackRate })}
      </button>

      {/* 画面去向（PRD-04-067）：只有视频笔记才有这枚。切换即写设置，所以"上次选的那样"
          跨笔记与重启都记得住——不是这一条上的临时开关。 */}
      {playbackState.mediaKind === "video" && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-now-playing__picture"
          aria-pressed={videoPrefs.backgroundVideo === "float"}
          title={t("nowPlaying.pictureHint")}
          onClick={() =>
            videoPrefs.setBackgroundVideo(
              videoPrefs.backgroundVideo === "float" ? "audio_only" : "float",
            )
          }
        >
          {t(
            videoPrefs.backgroundVideo === "float"
              ? "nowPlaying.pictureHide"
              : "nowPlaying.pictureShow",
          )}
        </button>
      )}

      {/* 「跟随」只在这一篇正开在编辑器里时出现——别处无处可跟，一枚按下去什么也不
          发生的开关比没有它更糟。默认关：编辑中被抢走滚动位置是最恼人的一种打扰。 */}
      {editorShowsPlayingNote && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-now-playing__follow"
          aria-pressed={followInEditor}
          title={t("nowPlaying.followHint")}
          onClick={() => setFollowInEditor(!followInEditor)}
        >
          {t("nowPlaying.follow")}
        </button>
      )}

      {reopenHref !== null && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-now-playing__reopen"
          onClick={() => {
            goTo(reopenHref);
          }}
        >
          {t("nowPlaying.reopen")}
        </button>
      )}

      <IconButton
        density="sm"
        className="luna-now-playing__close"
        label={t("nowPlaying.close")}
        icon={<CloseIcon size={12} />}
        onClick={close}
      />
    </div>
  );
}
