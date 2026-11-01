/**
 * 信息行（说明列 | 数值列）——「标签 : 值」在全应用的**唯一一枚**形态。
 *
 * 自铺两列网格会让同一屏里出现两种字号与两种行距；这里此前有两份逐字相同的实现，
 * 两处的注释都写着「与设置面板的事实行同一枚原语」——话是对的，实现不是。
 */

import type { ReactNode } from "react";

export interface FactRowProps {
  label: string;
  /** 值列。进度条一类的富内容也走这里，免得为它另铺一种两列网格（ADR-121 ⑤）。 */
  value: ReactNode;
  /** 标签下的副信息，如未登记权益的原始 key（ADR-121 ③）。 */
  hint?: string;
  /** 需要读屏播报变化时给 `status`（如"正在连接"这类会自己更新的值）。 */
  valueRole?: "status";
}

export function FactRow({ label, value, hint, valueRole }: FactRowProps) {
  return (
    <div className="luna-settings-panel__row">
      <div className="luna-settings-panel__row-label">
        <span className="luna-settings-panel__row-title">{label}</span>
        {hint ? <p className="luna-settings-panel__row-desc">{hint}</p> : null}
      </div>
      <p className="luna-settings-panel__row-value" role={valueRole}>
        {value}
      </p>
    </div>
  );
}
