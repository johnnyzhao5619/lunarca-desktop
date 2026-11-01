/**
 * 编辑器宽度分档（PRD-06-014「页头收口」P1 响应式三档）。
 *
 * 页头动作区、格式工具栏与「更多」菜单**按同一档位**降级：
 * - `wide`    ：AI / 全文朗读 / 版本历史是带文字的按钮；工具栏全部展开。
 * - `compact` ：这三颗降为图标钮（名字进提示）；工具栏不变。
 * - `narrow`  ：全文朗读与版本历史收进「更多」菜单，页头只留 AI 与「更多」；
 *               工具栏的 wiki 链接与「插入」组收进工具栏尾部的「更多格式」菜单。
 *
 * **判据在 JS 而不是 CSS 容器查询**：仓库其余处（设置页 / 搜索 / 回放）用的是
 * `@container`，那些只改布局。这里降级的一半是**菜单的内容**（narrow 时朗读与版本历史
 * 要出现在「更多」里），菜单经 portal 挂在 body 下，容器查询够不着它；把阈值一半写在
 * CSS、一半写在 JS 必然漂移。故阈值只在这里定义一次，组件据此渲染，CSS 只消费
 * `data-tier` 做间距微调。
 *
 * 阈值按最宽的一种页头量（录音笔记 + 英文界面：资料 / 同步回放 / 知识笔记三枚徽标 +
 * 段控 + 三颗文字钮 + 更多）：文字钮版本约需 880px 才不换行；图标钮版本约 600px。
 * `flex-wrap` 仍在——阈值判错时退化为换行，不会裁掉任何动作。
 */
export type EditorTier = "wide" | "compact" | "narrow";

export const EDITOR_TIER_WIDE_MIN = 880;
export const EDITOR_TIER_COMPACT_MIN = 600;

export function editorTierFor(inlineSize: number): EditorTier {
  if (inlineSize >= EDITOR_TIER_WIDE_MIN) {
    return "wide";
  }
  if (inlineSize >= EDITOR_TIER_COMPACT_MIN) {
    return "compact";
  }
  return "narrow";
}
