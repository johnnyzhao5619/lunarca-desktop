// 时长格式化——**两种呈现，各有其用，都在这一个文件里**。
//
// 收编前有四份：录音提示（`recording-cue`）、转写列表（`transcribe-list`）、语音 HUD
// （组件内私有的 `formatElapsed`）各写一份"时钟式"，本文件写一份"单位式"。前三份形状
// 看着一样，实则不一样——语音 HUD 那份**分钟带前导零且完全不处理小时**，于是录到一小时
// 十二分时，语音 HUD 念作 `72:15`，隔壁录音 HUD 念作 `1:12:15`。同一件事、同一屏、两种读法。

/**
 * 时钟式（`m:ss` / `h:mm:ss`）：**正在走的时间与媒体位置**——录音计时、语音计时、
 * 回放进度、时长徽标。分钟不补前导零（`1:05` 而不是 `01:05`），满一小时才出现小时段。
 *
 * 非有限/负数 → `"0:00"`（调用方若想在"未知"时干脆不渲染，自己判空，不要让格式化撒谎）。
 */
export function formatClock(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) {
    return "0:00";
  }
  const total = Math.max(0, Math.floor(ms / 1000));
  const seconds = total % 60;
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  const pad = (n: number) => String(n).padStart(2, "0");
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`;
}

/**
 * 单位式（`0m 27s` / `1h 3m 5s`）：**累计用量**——仪表盘听写时长、活动页音频用量。
 * 与时钟式的区别不是好看不好看：用量是"一共多久"，读作"三分二十七秒"；时钟是"到哪儿了"，
 * 读作"三比二十七"。两种问题，两种答案，故并存。
 */
export function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0 ? `${h}h ${m}m ${s}s` : `${m}m ${s}s`;
}
