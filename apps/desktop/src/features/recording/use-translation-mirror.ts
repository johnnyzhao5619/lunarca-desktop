/**
 * 译文独立窗控制 Hook（PRD-04-044，录音 HUD 与实时字幕面板共用，DRY）。
 *
 * 会话表面（录音 HUD / 字幕面板）持有麦克风会话与实时数据；本 Hook 把「独立窗偏好读写 +
 * 镜像视图广播 + 窗口开合」收敛为单一实现：
 * - 偏好按表面分键；广播携带来源与会话身份，关闭时只回写当前来源。
 * - 镜像视图行数恒随字幕外观 `caption.max_lines`（译文窗的呈现由 caption.* 外观统一驱动，
 *   与来源表面自身的显示行数无关）。
 * - 后端单会话约束（同一时刻只有一个录音/字幕会话）保证只有活动表面在持续广播，无争抢。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { commands, events } from "../../ipc/bindings";
import { buildSubtitleView, type SubtitleView } from "../recording/live-subtitle";
import type {
  HypothesisLine,
  RecordingMode,
  TranscriptLine,
  TranslationEntry,
} from "../recording/use-recording";
import { useCaptionAppearance } from "./use-caption-appearance";
import { CAPTION_VIEW_EVENT, type CaptionViewPayload } from "./caption-view-event";

/** 表面 → 独立窗偏好设置键。 */
export const MIRROR_SETTING_KEYS: Record<RecordingMode, string> = {
  record: "recording.separate_translation",
  caption: "caption.separate_translation",
};

export interface TranslationMirror {
  /** 本表面的「译文独立窗」偏好（持久设置的 UI 镜像）。 */
  separate: boolean;
  /** 切换偏好：写设置持久化；开启即时开窗（空态占位可见，便于先行摆放位置）。 */
  setSeparate: (next: boolean) => void;
  /** 按字幕外观行数构建的视图（开启独立窗时广播给译文窗；字幕面板亦直接渲染它）。 */
  view: SubtitleView;
}

export function useTranslationMirror(
  mode: RecordingMode,
  transcript: TranscriptLine[],
  hypotheses: HypothesisLine[],
  translations: TranslationEntry[],
  active: boolean,
  sessionId: string | null,
): TranslationMirror {
  const settingKey = MIRROR_SETTING_KEYS[mode];
  const [separate, setSeparateState] = useState(false);
  const wasActiveRef = useRef(false);
  const appearance = useCaptionAppearance();

  // 偏好初值 + 订阅热更新（译文窗 ✕ / 设置页改动都经 settings://changed 即时回流）。
  useEffect(() => {
    let cancelled = false;
    async function reload() {
      const result = await commands.listSettings();
      if (cancelled || result.status !== "ok") return;
      for (const spec of result.data) {
        if (spec.key === settingKey && spec.current_value.kind === "bool") {
          setSeparateState(spec.current_value.value);
        }
      }
    }
    void reload();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key === settingKey) void reload();
    });
    return () => {
      cancelled = true;
      void unlisten.then((fn) => fn());
    };
  }, [settingKey]);

  const view = useMemo(
    () => buildSubtitleView(transcript, hypotheses, translations, appearance.maxLines),
    [transcript, hypotheses, translations, appearance.maxLines],
  );

  // 预热窗口不能仅凭持久化偏好弹窗。只有活动来源或用户显式切换才能开合。
  useEffect(() => {
    if (active) {
      wasActiveRef.current = true;
      void (separate
        ? commands.captionTranslationShow()
        : commands.captionTranslationDismiss());
      return;
    }
    if (wasActiveRef.current) {
      wasActiveRef.current = false;
      void commands.captionTranslationDismiss();
    }
  }, [separate, active]);

  // 视图广播（开启期间随内容/会话态更新推送；译文窗被动渲染）。
  useEffect(() => {
    if (!separate) return;
    const payload: CaptionViewPayload = { source: mode, sessionId, view, active };
    void emit(CAPTION_VIEW_EVENT, payload);
  }, [separate, mode, sessionId, view, active]);

  function setSeparate(next: boolean) {
    setSeparateState(next); // 乐观更新（设置回执经订阅对齐）
    void commands.setSetting(settingKey, { kind: "bool", value: next });
    if (next) {
      const payload: CaptionViewPayload = { source: mode, sessionId, view, active };
      void emit(CAPTION_VIEW_EVENT, payload);
      void commands.captionTranslationShow();
    } else {
      void commands.captionTranslationDismiss();
    }
  }

  return { separate, setSeparate, view };
}
