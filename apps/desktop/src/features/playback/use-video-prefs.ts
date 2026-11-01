/**
 * 视频回放偏好（PRD-04-058）：布局 / 字幕开关 / 字幕字号。
 *
 * **设置是唯一事实源**（PRD-09）：本 Hook 只做「读设置 → 渲染」与「用户改动 → 写设置」，
 * 并订阅 `settings://changed` 重读——于是设置页与回放视图内的控件永远同步，且跨笔记、
 * 跨重启记忆（与编辑器写作模式 `editor.typewriter_mode` 同一 pattern，DRY）。
 */

import { useCallback, useEffect, useState } from "react";
import { commands, events } from "../../ipc/bindings";
import {
  DEFAULT_BACKGROUND_VIDEO,
  DEFAULT_SUBTITLE_SIZE,
  DEFAULT_VIDEO_LAYOUT,
  parseBackgroundVideo,
  parseSubtitleSize,
  parseVideoLayout,
  type BackgroundVideo,
  type SubtitleSize,
  type VideoLayout,
} from "./video-stage";

const LAYOUT_KEY = "playback.video_layout";
const SUBTITLES_KEY = "playback.video_subtitles";
const SIZE_KEY = "playback.subtitle_size";
const BACKGROUND_KEY = "playback.background_video";

export interface VideoPrefs {
  layout: VideoLayout;
  subtitlesEnabled: boolean;
  subtitleSize: SubtitleSize;
  /** 离开回放视图后画面的去向（PRD-04-067）。 */
  backgroundVideo: BackgroundVideo;
  setLayout: (layout: VideoLayout) => void;
  setSubtitlesEnabled: (enabled: boolean) => void;
  setSubtitleSize: (size: SubtitleSize) => void;
  setBackgroundVideo: (mode: BackgroundVideo) => void;
}

/**
 * @param enabled 是否真的去读设置。常驻播放宿主（ADR-118）在**空载**时挂着但不需要这些
 *   偏好，而它随工作区常驻——无条件读会给每次启动凭空加一次 `listSettings`。关闭时保持
 *   默认值，装载第一篇的那一刻再读。
 */
export function useVideoPrefs(enabled = true): VideoPrefs {
  const [layout, setLayoutState] = useState<VideoLayout>(DEFAULT_VIDEO_LAYOUT);
  const [subtitlesEnabled, setSubtitlesState] = useState(true);
  const [subtitleSize, setSubtitleSizeState] = useState<SubtitleSize>(DEFAULT_SUBTITLE_SIZE);
  const [backgroundVideo, setBackgroundVideoState] =
    useState<BackgroundVideo>(DEFAULT_BACKGROUND_VIDEO);

  const load = useCallback(async () => {
    const result = await commands.listSettings();
    if (result.status !== "ok") {
      return;
    }
    const value = (key: string) => result.data.find((spec) => spec.key === key)?.current_value;
    const layoutValue = value(LAYOUT_KEY);
    setLayoutState(parseVideoLayout(layoutValue?.kind === "str" ? layoutValue.value : null));
    const subtitles = value(SUBTITLES_KEY);
    if (subtitles?.kind === "bool") {
      setSubtitlesState(subtitles.value);
    }
    const size = value(SIZE_KEY);
    setSubtitleSizeState(parseSubtitleSize(size?.kind === "str" ? size.value : null));
    const background = value(BACKGROUND_KEY);
    setBackgroundVideoState(
      parseBackgroundVideo(background?.kind === "str" ? background.value : null),
    );
  }, []);

  useEffect(() => {
    if (!enabled) return;
    void load();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key.startsWith("playback.")) {
        void load();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [load, enabled]);

  // 乐观置位 + 写设置：设置写回会广播 settings://changed 使各表面收敛到同一值。
  const setLayout = useCallback((next: VideoLayout) => {
    setLayoutState(next);
    void commands.setSetting(LAYOUT_KEY, { kind: "str", value: next });
  }, []);

  const setSubtitlesEnabled = useCallback((next: boolean) => {
    setSubtitlesState(next);
    void commands.setSetting(SUBTITLES_KEY, { kind: "bool", value: next });
  }, []);

  const setSubtitleSize = useCallback((next: SubtitleSize) => {
    setSubtitleSizeState(next);
    void commands.setSetting(SIZE_KEY, { kind: "str", value: next });
  }, []);

  const setBackgroundVideo = useCallback((next: BackgroundVideo) => {
    setBackgroundVideoState(next);
    void commands.setSetting(BACKGROUND_KEY, { kind: "str", value: next });
  }, []);

  return {
    layout,
    subtitlesEnabled,
    subtitleSize,
    backgroundVideo,
    setLayout,
    setSubtitlesEnabled,
    setSubtitleSize,
    setBackgroundVideo,
  };
}
