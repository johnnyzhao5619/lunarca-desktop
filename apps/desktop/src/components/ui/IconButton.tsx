import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";

type NativeButtonProps = Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "aria-label" | "title" | "children" | "className"
>;

export interface IconButtonProps extends NativeButtonProps {
  /**
   * 这颗按钮做什么——**必填，一处写、两处用**：它同时是读屏念出的可访问名与鼠标悬停
   * 的工具提示。
   *
   * **收编前这两者是分开写的，于是漂移**：标签页栏四枚只写了 `title`、关闭那枚
   * 两个都写；对话页十三枚全只写 `title`。`title` 确实能兜底出可访问名，但它是最弱的
   * 一档来源，且"要写哪个"没有规则可循时下一个人只会照抄离得最近的那份。做成必填参数
   * 之后，忘不掉，也没得抄错。
   */
  label: string;
  /** 图标本体（`./Icon` 里的组件）。尺寸由调用方给，与按钮热区无关。 */
  icon: ReactNode;
  /**
   * 密度档：默认是界面框架（顶栏、面板头、标签页栏、对话页头与输入区）的一档；
   * `sm` 是**正文内容行内**的一档（消息悬停动作、历史条目动作）——那里按钮夹在文字
   * 之间，框架档会把行撑开；`hud` 是五扇悬浮窗的密度契约（`hud-density-guard`）。
   */
  density?: "sm" | "hud";
  /**
   * 语气：`danger` 用于破坏性动作与中断（常态与其余动作等重，只在 hover 显状态色——
   * 理由见 `icon-button.css`）；`primary` 用于**该区域唯一的主动作**（对话输入区的发送）。
   */
  tone?: "danger" | "primary";
  /** 附加类名（放置/间距等由所在区域负责的事）；按钮自身的样子一律走本原语。 */
  className?: string;
}

/**
 * 图标按钮（`.luna-icon-btn`）——全应用「一枚图标就是全部内容」那种按钮的唯一实现。
 *
 * 样式的收编见 `components/icon-button.css` 的抬头；本组件收编的是**行为与可访问名**：
 * 名字必填且只写一次，`type` 默认 `button`（漏写 `type` 的按钮在 `<form>` 里会变成提交
 * 键——对话页的发送区正好是个 form，其余图标钮混在里面），按下态一律走 `aria-pressed`
 * 而不是各写各的 `data-active`（裁决 D5）。
 */
export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, icon, density, tone, className, type = "button", ...rest },
  ref,
) {
  const classes = ["luna-icon-btn"];
  if (density) {
    classes.push(`luna-icon-btn--${density}`);
  }
  if (tone) {
    classes.push(`luna-icon-btn--${tone}`);
  }
  if (className) {
    classes.push(className);
  }

  return (
    <button
      {...rest}
      ref={ref}
      type={type}
      className={classes.join(" ")}
      aria-label={label}
      title={label}
    >
      {icon}
    </button>
  );
});
