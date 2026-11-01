/**
 * 书签确认条的自动收起策略（PRD-04-005；纯函数）。
 *
 * 此前点书签后 6 秒无条件收起，只有输入框**聚焦**才取消：用户看到提示、想了 3 秒再去点
 * 输入框，可能正好在点击前收起；而输入框里按 Esc 走的是「立刻收起」，已打的字直接丢。
 * 现在：备注**有内容**就不自动收；空着且没在编辑才倒计时。Esc 只失焦，提交或 ✕ 才关。
 */

export const BOOKMARK_TOAST_DISMISS_MS = 6_000;

export function bookmarkAutoDismissMs(input: { note: string; editing: boolean }): number | null {
  if (input.editing) return null;
  if (input.note.trim().length > 0) return null;
  return BOOKMARK_TOAST_DISMISS_MS;
}
