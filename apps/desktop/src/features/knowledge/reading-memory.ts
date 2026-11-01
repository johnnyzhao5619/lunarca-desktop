/**
 * 知识产物阅读视图的滚动位置记忆（K5「返回主题保留位置」）。
 *
 * 点出处会切到笔记空间、阅读视图随之卸载；回来时按路径恢复上次读到的位置，而不是回到
 * 页首。只在本次运行的内存里：跨启动恢复阅读位置不是这里的承诺，也不落盘（「继续阅读」
 * 哪一页由 Rust 记在库内界面现场，见 `record_knowledge_visit`）。
 * 上限防止长时间使用后无界增长——最久没碰的先忘。
 */
const LIMIT = 50;
const memory = new Map<string, number>();

export function rememberReadingScroll(path: string, top: number): void {
  memory.delete(path);
  memory.set(path, top);
  if (memory.size > LIMIT) {
    const oldest = memory.keys().next().value;
    if (oldest !== undefined) memory.delete(oldest);
  }
}

export function recallReadingScroll(path: string): number | undefined {
  return memory.get(path);
}
