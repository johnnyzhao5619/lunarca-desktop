/**
 * 把目标滚到**它所在的滚动窗格**里看得见的位置——只动最近的可滚动祖先。
 *
 * 不用 `scrollIntoView`：它会一路滚动**每一层**祖先，包括 `overflow: hidden` 的应用外壳
 * ——那一层本不该滚，但程序式滚动照样生效。真机 2026-09-07：算力卡片「更改」深链到模型
 * 分配槽后，整个窗口内容上移、状态栏下面露出一块空白，切页才恢复。
 */
export function scrollWithinPane(element: HTMLElement): void {
  let pane: HTMLElement | null = element.parentElement;
  while (pane) {
    const overflowY = window.getComputedStyle(pane).overflowY;
    if ((overflowY === "auto" || overflowY === "scroll") && pane.scrollHeight > pane.clientHeight) {
      break;
    }
    pane = pane.parentElement;
  }
  if (!pane) return;
  const paneRect = pane.getBoundingClientRect();
  const rect = element.getBoundingClientRect();
  const offset = rect.top - paneRect.top + pane.scrollTop;
  pane.scrollTo({ top: paneScrollTop(offset, pane.clientHeight, rect.height) });
}

/**
 * 目标在窗格内容里的顶端位置 → 窗格该滚到哪：放得下就居中（一行设置），放不下就让它的
 * **顶端**贴着窗格顶（一整节设置比窗格还高）。此前一律居中，高出窗格的那一节居中后连节标题
 * 带头两行都被推到视口上方——真机 2026-10-06：设置页点「编辑器」锚点，看到的是第三行。
 */
export function paneScrollTop(offset: number, paneHeight: number, elementHeight: number): number {
  const centered = offset - (paneHeight - elementHeight) / 2;
  return Math.max(0, Math.min(offset, centered));
}
