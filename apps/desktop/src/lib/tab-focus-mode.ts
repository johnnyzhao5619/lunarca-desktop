/**
 * 「Tab 键移动焦点」（会话级开关，VS Code 的 Toggle Tab Key Moves Focus 同义）。
 *
 * 编辑器里 Tab 是缩进（写作工具惯例），键盘用户要离开正文有两条路：Esc 后 2 秒内 Tab（CM6 自带），
 * 或打开本开关（⌘⇧M / 命令面板）让 Tab 回到「移动焦点」。开关是**一份**：所有编辑器跟它走，状态栏
 * 读它显示指示——CodeMirror 自带的 ⇧⌥M 只切当前那一个编辑器，各 pane 各一份就会与状态栏对不上，
 * 故由编辑器把这两颗键都接到这里（WCAG 2.1.2 无键盘陷阱：出路要有、要看得见）。不持久化：它是这一次的
 * 操作方式，不是偏好。
 */

import { useSyncExternalStore } from "react";

let enabled = false;
const listeners = new Set<() => void>();

export function tabFocusModeEnabled(): boolean {
  return enabled;
}

export function setTabFocusMode(next: boolean): void {
  if (next === enabled) return;
  enabled = next;
  for (const listener of listeners) listener();
}

export function toggleTabFocusMode(): void {
  setTabFocusMode(!enabled);
}

export function subscribeTabFocusMode(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useTabFocusMode(): boolean {
  return useSyncExternalStore(subscribeTabFocusMode, tabFocusModeEnabled, tabFocusModeEnabled);
}
