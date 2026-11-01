/**
 * 录音 HUD 的非阻断横幅**只挂一条**（PRD-04-003 修订 2026-09-07；纯函数）。
 *
 * 此前四条横幅各自独立渲染：转写掉线与重连做了互斥，但转译掉线与负载降级与前两者
 * 无互斥——420px 宽的窗里叠三条 30+ 字的长句，把字幕画布挤到几乎没有，再触发自动扩容
 * 把窗口撑高。现在按优先级取最高的一条上屏，其余折进它的悬停说明。
 *
 * 优先级（高 → 低）：转写掉线 > 重连中 > 转译掉线（仅转译开着时算）> 负载降级。
 * 「转写掉线」压过「重连中」是既有裁决：两条同时挂着会互相矛盾（已停止 vs 正在重连）。
 */

export type RecordingBannerKind =
  | "transcribe_stopped"
  | "transcribe_reconnecting"
  | "translate_stopped"
  | "degraded";

export type RecordingBanner = {
  kind: RecordingBannerKind;
  /** `data-tier`：样式档（stopped / reconnecting / yellow / orange / red）。 */
  tier: string;
  /** 首选文案 key（`recording` 命名空间）。 */
  key: string;
  /** 首选缺失时的兜底 key（稳定码表会随服务端演进增加，认不出的码给准确的通用句）。 */
  fallbackKey: string;
};

export type RecordingBannerInput = {
  transcribeStopped: string | null;
  transcribeReconnecting: boolean;
  translateStopped: string | null;
  translateEnabled: boolean;
  degradedTier: string | null;
  degradedReason: string | null;
};

/** 全部候选横幅，按优先级排好；`shown` = 第一条，`folded` = 其余。 */
export function recordingBanners(input: RecordingBannerInput): RecordingBanner[] {
  const banners: RecordingBanner[] = [];
  if (input.transcribeStopped) {
    banners.push({
      kind: "transcribe_stopped",
      tier: "stopped",
      key: `transcribe_stopped.${input.transcribeStopped.toLowerCase()}`,
      fallbackKey: "transcribe_stopped.generic",
    });
  } else if (input.transcribeReconnecting) {
    banners.push({
      kind: "transcribe_reconnecting",
      tier: "reconnecting",
      key: "transcribe_reconnecting",
      fallbackKey: "transcribe_reconnecting",
    });
  }
  // 译文开着才算：关着的时候「转译停了」不是一件用户关心的事。
  if (input.translateStopped && input.translateEnabled) {
    banners.push({
      kind: "translate_stopped",
      tier: "stopped",
      key: `translate_stopped.${input.translateStopped.toLowerCase()}`,
      fallbackKey: "translate_stopped.generic",
    });
  }
  if (input.degradedTier && input.degradedTier !== "green") {
    banners.push({
      kind: "degraded",
      tier: input.degradedTier,
      key: input.degradedReason
        ? `degraded.${input.degradedReason}`
        : `degraded.${input.degradedTier}`,
      fallbackKey: `degraded.${input.degradedTier}`,
    });
  }
  return banners;
}

export function pickRecordingBanner(input: RecordingBannerInput): {
  shown: RecordingBanner | null;
  folded: RecordingBanner[];
} {
  const [shown = null, ...folded] = recordingBanners(input);
  return { shown, folded };
}
