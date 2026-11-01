export type VoiceLevel = {
  peak: number;
  rms: number;
};

export type VoiceLevelPayload = ArrayBuffer | Uint8Array | number[];

const QUIET_LEVEL: VoiceLevel = { peak: 0, rms: 0 };

function unit(byte: number | undefined): number {
  return Math.max(0, Math.min(1, (byte ?? 0) / 255));
}

function toBytes(payload: VoiceLevelPayload): Uint8Array | number[] {
  if (payload instanceof ArrayBuffer) {
    return new Uint8Array(payload);
  }
  if (payload instanceof Uint8Array) {
    return payload;
  }
  return payload;
}

// 一个 Level 事件是 Rust 侧 `encode_levels` 产出的扁平字节流：每帧 2 字节 `[peak, rms]`，
// 一个事件可含 N 帧（ADR-011）。必须解出**全部**帧，不能只取第一帧（否则丢波形）。
export function decodeVoiceLevels(payload: VoiceLevelPayload): VoiceLevel[] {
  const bytes = toBytes(payload);
  const out: VoiceLevel[] = [];
  for (let i = 0; i + 1 < bytes.length; i += 2) {
    out.push({ peak: unit(bytes[i]), rms: unit(bytes[i + 1]) });
  }
  return out;
}

/** 录音电平通道的轨 id（Rust `LEVEL_TRACK_*` 同义，TDD-02 §6 双轨电平）。 */
export const LEVEL_TRACK_PRIMARY = 0;
export const LEVEL_TRACK_SYSTEM = 1;

// 录音通道消息 = [轨 id, peak, rms, …]（Rust `tag_track_levels`；一条消息恒单轨）。
// 语音输入通道仍为无前缀扁平格式（`decodeVoiceLevels`）——两通道格式独立。
export function decodeTrackLevels(payload: VoiceLevelPayload): {
  track: number;
  levels: VoiceLevel[];
} {
  const bytes = toBytes(payload);
  if (bytes.length === 0) {
    return { track: LEVEL_TRACK_PRIMARY, levels: [] };
  }
  const track = bytes[0] ?? LEVEL_TRACK_PRIMARY;
  const out: VoiceLevel[] = [];
  for (let i = 1; i + 1 < bytes.length; i += 2) {
    out.push({ peak: unit(bytes[i]), rms: unit(bytes[i + 1]) });
  }
  return { track, levels: out };
}

/** 追加一批已解码帧到滚动缓冲（容量裁剪与 `pushLevelFrames` 同口径）。 */
export function appendLevelFrames(
  frames: VoiceLevel[],
  decoded: VoiceLevel[],
  maxFrames: number,
): VoiceLevel[] {
  const next = [...frames, ...decoded];
  return next.slice(Math.max(0, next.length - maxFrames));
}

export function voiceLevelBucket(level: VoiceLevel): number {
  // 条高用 **rms（语音能量包络）**：随说话强弱起伏，动态可见。
  // 显示映射使用对数增益，让低声讲话脱离 idle floor，同时中等 rms 不被 peak 钉满。
  if (level.rms <= 0) {
    return level.peak > 0 ? 1 : 0;
  }
  const normalized = Math.log1p(Math.max(0, level.rms) * 220) / Math.log1p(220);
  return Math.max(1, Math.min(10, Math.ceil(normalized * 10)));
}

export function pushLevelFrames(
  frames: VoiceLevel[],
  batch: VoiceLevelPayload,
  maxFrames: number,
): VoiceLevel[] {
  const next = [...frames, ...decodeVoiceLevels(batch)];
  return next.slice(Math.max(0, next.length - maxFrames));
}

function mergeLevelFrames(frames: VoiceLevel[]): VoiceLevel {
  return frames.reduce<VoiceLevel>(
    (acc, frame) => ({
      peak: Math.max(acc.peak, frame.peak),
      rms: Math.max(acc.rms, frame.rms),
    }),
    QUIET_LEVEL,
  );
}

export function compactVoiceLevels(frames: VoiceLevel[], displayFrames: number): VoiceLevel[] {
  const target = Math.max(0, Math.floor(displayFrames));
  if (target === 0) {
    return [];
  }
  if (frames.length === 0) {
    return Array.from<VoiceLevel>({ length: target }).fill(QUIET_LEVEL);
  }
  if (frames.length <= target) {
    return [
      ...Array.from<VoiceLevel>({ length: target - frames.length }).fill(QUIET_LEVEL),
      ...frames,
    ];
  }

  const bucketSize = frames.length / target;
  return Array.from({ length: target }, (_, index) => {
    const start = Math.floor(index * bucketSize);
    const end = Math.max(start + 1, Math.floor((index + 1) * bucketSize));
    return mergeLevelFrames(frames.slice(start, end));
  });
}
