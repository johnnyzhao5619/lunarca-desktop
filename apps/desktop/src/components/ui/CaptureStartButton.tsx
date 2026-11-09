/**
 * 采集侧栏的主动作按钮（`.luna-capture-start`）——录音 / 转写 / 速记 / AI 对话四条侧栏
 * 同一枚（PRD-07-071 ④）。
 *
 * 收成组件而不是只共用一个类名：那四处此前各写一遍同样的
 * `<button className="luna-btn luna-btn--primary luna-capture-start">图标 + 文字</button>`，
 * 而**快捷键提示只有速记那一颗有**——用户在录音与对话那两格里根本看不出这两件事也能
 * 在别的应用里按（真机 2026-09-09）。挂在按钮上而不是另起一行脚注：这条提示的价值是
 * 「不切回 Lunarca 也能按」，与空态要讲的事不同，挤在一起谁也讲不清。
 *
 * **`shortcut` 收已经格式化好的展示串**（`formatShortcut`），不收原始注册表值：
 * 「什么算一个可读的组合」在 `@lunarca/editor-core/shortcut-display` 有唯一实现，这里再解析一次就是
 * 第二处会漂的定义。未绑定就传空串——不显示，也不在名字里提。
 */

import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

export interface CaptureStartButtonProps {
  /** 按钮上的图标（尺寸由调用方给：与文字同层，不是热区）。 */
  icon: ReactNode;
  /** 按钮文字，也是读屏名的主体。 */
  label: string;
  /** 已格式化的快捷键展示串；空串 = 未绑定，不渲染。 */
  shortcut?: string;
  onClick: () => void;
  disabled?: boolean;
  /** 附加类名（拆分按钮的左半等排版事项由所在区域负责）。 */
  className?: string;
}

export function CaptureStartButton({
  icon,
  label,
  shortcut = "",
  onClick,
  disabled,
  className,
}: CaptureStartButtonProps) {
  const { t } = useTranslation("common");
  // 读屏名带上快捷键：`kbd` 是 `aria-hidden` 的装饰，名字里不提它就等于没告诉读屏用户。
  const accessibleName = shortcut ? t("action.withShortcut", { action: label, shortcut }) : label;
  const classes = ["luna-btn", "luna-btn--primary", "luna-capture-start"];
  if (className) {
    classes.push(className);
  }
  return (
    <button
      type="button"
      className={classes.join(" ")}
      aria-label={accessibleName}
      title={accessibleName}
      disabled={disabled}
      onClick={onClick}
    >
      <span className="luna-capture-start__body">
        <span className="luna-capture-start__main">
          {icon}
          <span className="luna-capture-start__label">{label}</span>
        </span>
        {shortcut ? (
          <kbd className="luna-capture-start__kbd" aria-hidden="true">
            {shortcut}
          </kbd>
        ) : null}
      </span>
    </button>
  );
}
