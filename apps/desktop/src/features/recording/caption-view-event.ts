/**
 * 原文字幕窗 → 译文独立窗的视图广播（PRD-04-044）。
 *
 * 麦克风会话由原文窗单一持有（`useRecording` mode caption），译文窗是**被动镜像**：原文窗把
 * 构建好的字幕视图经 Tauri 全局事件广播，译文窗监听后仅渲染译文行——避免二次抢麦或重复会话。
 */

import type { SubtitleView } from "../recording/live-subtitle";
import type { RecordingMode } from "./use-recording";

export const CAPTION_VIEW_EVENT = "caption://view";

export interface CaptionViewPayload {
  /** 发出镜像的业务表面，用于关闭时只回写对应设置。 */
  source: RecordingMode;
  /** 会话身份；空闲预览为 null。 */
  sessionId: string | null;
  view: SubtitleView;
  /** 会话是否进行中（译文窗据此显示聆听/空态）。 */
  active: boolean;
}
