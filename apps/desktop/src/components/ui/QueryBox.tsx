import type { KeyboardEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CloseIcon, SearchIcon } from "@lunarca/editor-core/Icon";
import { isImeKeyEvent } from "../../lib/ime-key-event";

export interface QueryBoxProps {
  value: string;
  onChange: (value: string) => void;
  /** 占位提示（面板自己的措辞，如「搜索笔记…」）。 */
  placeholder: string;
  /**
   * 输入框的可访问名。必填——查询框只有一枚放大镜图标，没有可见标签；
   * 缺了它读屏只念得出「编辑框」，说不清是在搜什么。
   */
  ariaLabel: string;
  /** 框内额外动作（如搜索面板的大小写敏感开关），排在清空钮之后。 */
  children?: ReactNode;
  className?: string;
  /** 目的地级搜索可在挂载时直接把键盘交给查询框。 */
  autoFocus?: boolean;
  /**
   * 框内键盘语义（如回放查找的 Enter 跳下一处、Esc 清空）；不给则只是普通输入框。
   * 输入法组字与上屏那一下不会转给它（PRD-07-040 ①）——调用方不必各自再判。
   */
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
}

/**
 * 面板内查询框（`.luna-query-box`）——侧栏与全宽页面头部「在这一堆里找」的唯一实现。
 *
 * 收编前六个面板各写一份：搜索是复合框、图谱借用搜索的类名、标签与回收站是裸输入框、
 * 动态中心两个面板连类名都没有、设置侧栏另有一份。差异不只是好看不好看——**有没有
 * 清空按钮**、**读屏念不念得出这是搜什么的**，六处各不相同。
 *
 * 清空钮只在有内容时出现（空框上的 ✕ 没有可点的对象），并始终带 `aria-label`。
 */
export function QueryBox({
  value,
  onChange,
  placeholder,
  ariaLabel,
  children,
  className,
  autoFocus = false,
  onKeyDown,
}: QueryBoxProps) {
  const { t: tCommon } = useTranslation("common");
  const clearLabel = tCommon("action.clearQuery");

  return (
    <div className={className ? `luna-query-box ${className}` : "luna-query-box"}>
      <SearchIcon size={14} className="luna-query-box__icon" />
      <input
        type="text"
        className="luna-query-box__input"
        value={value}
        placeholder={placeholder}
        aria-label={ariaLabel}
        autoFocus={autoFocus}
        onChange={(event) => onChange(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (!isImeKeyEvent(event)) onKeyDown?.(event);
        }}
      />
      {value.length > 0 && (
        <button
          type="button"
          className="luna-query-box__action"
          title={clearLabel}
          aria-label={clearLabel}
          onClick={() => onChange("")}
        >
          <CloseIcon size={12} />
        </button>
      )}
      {children}
    </div>
  );
}
