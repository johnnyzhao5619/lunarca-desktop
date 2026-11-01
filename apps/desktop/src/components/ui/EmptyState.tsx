import { useRef, type ReactNode } from "react";
import { useScrollEdges } from "../../lib/scroll-edges";

export interface EmptyStateProps {
  /** 主图标（如 `SearchIcon` / `FolderIcon`）。 */
  icon?: ReactNode;
  /** 主标题：说清**当前是什么状况**，不要复述面板叫什么名字。 */
  title: string;
  /** 说明或下一步提示。 */
  description?: string;
  /** 主动作（Call to Action）：空态的价值一半在这里——只说"这里是空的"等于把人晾着。 */
  action?: ReactNode;
  /**
   * 补充脚注：比说明更轻的一行，可含 `<kbd>`（如「按 ⌘⇧V 开始」）或格式说明。
   *
   * 它替掉了原先的 `shortcuts: string[]`——那个 API **全仓无人使用**，而真实需求恰恰是
   * 它表达不了的那种：一句话里嵌一枚按键，而不是一排孤零零的键帽。
   */
  footnote?: ReactNode;
  /** 额外内容块（如搜索空态里的语法速查）。跟在动作之后，随空态一起滚动。 */
  children?: ReactNode;
  className?: string;
}

/**
 * Lunarca 标准空状态。**全应用唯一实现**——此前并存四套：本组件、采集族的
 * `.luna-capture-empty`（速记/录音/转写/副库四处）、搜索的 `.luna-search__empty`、
 * 对话的 `.luna-chat__empty`，外加若干处裸 `<p>`。四套各自演化后连键帽的圆角与阴影
 * 都不一样了，更要紧的是**有没有"下一步"这一项**完全看运气。
 *
 * 收敛时把各家真正需要的槽位补进本组件（`footnote` / `children` / 可滚动），而不是
 * 让调用方因为缺一个槽位就再分叉一份。
 */
export function EmptyState({
  icon,
  title,
  description,
  action,
  footnote,
  children,
  className = "",
}: EmptyStateProps) {
  // 空态自己滚（内容超高时）：长说明被裁在半句、下面的主动作看不见时，底边渐隐提示「下面还有」。
  const rootRef = useRef<HTMLDivElement>(null);
  const edges = useScrollEdges(rootRef, [title, description]);
  return (
    <div
      ref={rootRef}
      className={`luna-empty-state luna-scroll-fade ${className}`}
      data-scroll-start={edges.start || undefined}
      data-scroll-end={edges.end || undefined}
    >
      {icon && <div className="luna-empty-state__icon">{icon}</div>}
      <h3 className="luna-empty-state__title">{title}</h3>
      {description && <p className="luna-empty-state__description">{description}</p>}
      {footnote && <p className="luna-empty-state__footnote">{footnote}</p>}
      {action && <div className="luna-empty-state__action">{action}</div>}
      {children}
    </div>
  );
}
