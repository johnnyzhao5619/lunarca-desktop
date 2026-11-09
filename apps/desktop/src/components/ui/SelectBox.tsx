import type { ReactNode, SelectHTMLAttributes } from "react";
import { ChevronDownIcon } from "@lunarca/editor-core/Icon";

export interface SelectBoxProps
  extends Omit<SelectHTMLAttributes<HTMLSelectElement>, "className" | "size"> {
  /**
   * 控件的可访问名。必填——下拉常常只有一枚图标或一段前缀文字作为可见标签，
   * 缺了它读屏只念得出当前选中项，说不清这是在选什么。
   * 若已有可见 `<label for>`，传 `labelledBy` 而不是这一项。
   */
  ariaLabel?: string;
  /** 已有可见标签时用它关联，与 `ariaLabel` 二选一。 */
  labelledBy?: string;
  /** 尺寸：`md`（默认，独立字段）/ `sm`（嵌在工具条里的档位）。 */
  size?: "md" | "sm";
  /** 外框的额外类：**只放这一行里怎么占位**（flex 基准、最大宽度），不放外观。 */
  className?: string;
  /** `<option>` 列表。 */
  children: ReactNode;
}

/**
 * 下拉选择（`.luna-select`）——全应用唯一实现，外观见 `components/select.css`。
 *
 * **它是"外框 + 内部裸控件"而不是给 `<select>` 加一个类**：原生 `<select>` 关掉外观后
 * 就没有下拉箭头了，而箭头必须是应用自己的 chevron（与文件树段头同一枚），不能是 UA 的
 * 三角，也不能是写死颜色的 `background-image`（那会绕过 Token）。外框同时让它与
 * `.luna-query-box` 成为同一个盒模型——同一行里等高是结构保证的，不靠手调内边距。
 *
 * 定位类（`flex` 基准 / 最大宽度）落在**外框**上：内部控件恒为 `flex: 1 1 0`。
 */
export function SelectBox({
  ariaLabel,
  labelledBy,
  size = "md",
  className,
  children,
  ...selectProps
}: SelectBoxProps) {
  const frameClasses = ["luna-select"];
  if (size === "sm") frameClasses.push("luna-select--sm");
  if (className) frameClasses.push(className);

  return (
    <span className={frameClasses.join(" ")}>
      <select
        {...selectProps}
        className="luna-select__control"
        aria-label={ariaLabel}
        aria-labelledby={labelledBy}
      >
        {children}
      </select>
      <ChevronDownIcon size={size === "sm" ? 12 : 14} className="luna-select__chevron" />
    </span>
  );
}
