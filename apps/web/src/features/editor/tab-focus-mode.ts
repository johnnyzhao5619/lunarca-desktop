/**
 * 「Tab 键移动焦点」（WCAG 2.1.2 键盘出路）：本页会话级的一份开关，所有编辑器跟它走（⌘⇧M / ⇧⌥M 切换，
 * 键位由共享编辑器接到 `EditorHost.toggleTabFocusMode`）。不落盘：刷新页面回到默认关。
 */

let enabled = false;
const listeners = new Set<() => void>();

export function tabFocusModeEnabled(): boolean {
  return enabled;
}

export function toggleTabFocusMode(): void {
  enabled = !enabled;
  for (const listener of listeners) listener();
}

export function subscribeTabFocusMode(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
