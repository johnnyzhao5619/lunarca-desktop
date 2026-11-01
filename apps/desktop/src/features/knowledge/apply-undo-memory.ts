/**
 * 主题页「撤销这次写入」的会话记忆（K4「撤销错误不需要自行整理」）。
 *
 * 写入落定后面板给「撤销这次写入」；此前凭据只在面板组件里，离开主题再回来就没了，
 * 只剩版本历史这条绕路（2026-09-27 真机）。这里按主题路径记下本次运行里确认的那次写入的
 * 前后指纹，回来时接着给撤销入口。只在内存、不落盘：撤销能不能做仍由 Rust 按「此刻
 * 页面是否仍是写入后那一版」判定，之后改过就如实说改过，不会覆盖。
 * 上限防止长时间使用后无界增长——最久没碰的先忘。
 */

export interface ApplyUndo {
  before: string;
  after: string;
}

const LIMIT = 20;
const memory = new Map<string, ApplyUndo>();

export function rememberApplyUndo(path: string, undo: ApplyUndo): void {
  memory.delete(path);
  memory.set(path, undo);
  if (memory.size > LIMIT) {
    const oldest = memory.keys().next().value;
    if (oldest !== undefined) memory.delete(oldest);
  }
}

export function recallApplyUndo(path: string): ApplyUndo | undefined {
  return memory.get(path);
}

export function forgetApplyUndo(path: string): void {
  memory.delete(path);
}
