import { type HTMLAttributes, type ReactNode } from "react";
import { TaskStatus, type TaskStatusKind } from "../TaskStatus";
import { HudUplinkDot } from "./HudUplinkDot";

type NativeHeaderProps = Omit<HTMLAttributes<HTMLElement>, "title" | "children">;

export interface HudHeaderProps extends NativeHeaderProps {
  /**
   * 身份图标——**每扇窗一枚、两两不同**（ADR-111 ①：弹窗翻译与实时字幕曾共用同一枚）。
   * 翻译 Languages · 语音 Mic · 录音 Disc · 字幕 Captions · 对话 Sparkles；统一 15px。
   */
  icon: ReactNode;
  /**
   * 身份位上的主控件（ADR-112 ② 的「身份/状态合一的脉冲点」）：给了它就由它占身份位，
   * `icon` 不再单独画——语音胶囊的说话圆钮本身就是一枚麦克风，旁边再摆一枚是同一件事说两遍。
   * `icon` 仍必填：它是这扇窗的身份声明（门禁按它查两两不同）。
   */
  lead?: ReactNode;
  /** 窗口名：只给读屏（ADR-137 V2）——视觉上身份由图标说，每次都一样的名字不占地方。 */
  title: string;
  /** 配置胶囊：**这一场的配置**（语音的模式、对话的临时/角色）。唯一皮 `.luna-hud-context-badge`。 */
  context?: ReactNode;
  /** 状态胶囊：**现在在做什么**。唯一皮 `.luna-task-status`——不接受自绘状态元素。 */
  status?: { kind: TaskStatusKind; label: string };
  /**
   * 联网指示（ADR-038 ④）：`leavesDevice` 为真即渲染，任何形态不许省。
   * `active`（默认真）= 此刻正在采集；空闲态传假，指示描边、文案改为「说话时将交给…」。
   */
  uplink?: { leavesDevice: boolean; name: string | null; active?: boolean } | null;
  /**
   * 与状态同排的内容（ADR-137 V2「与内容同排而非独占一行」）：占满动作区之前的余宽，
   * 胶囊档的实时文字、提示语放在这里，窗口就只有一行。
   */
  lane?: ReactNode;
  /** 右缘窗口动作（`data-no-drag` 由本组件统一挂）。 */
  actions?: ReactNode;
  /** 锚定在窗口栏上的浮层（对话面板的历史弹层）——渲染在动作区之后。 */
  children?: ReactNode;
}

/**
 * HUD 状态行（ADR-111 ② / ADR-137 V2）——五槽契约的唯一实现：
 *
 *   [身份图标] [状态胶囊] [配置胶囊] [联网点] ····· [窗口动作]
 *
 * 状态在前：「现在在做什么」是这一行最该被读到的，配置是它的修饰。标题文字只进无障碍树。
 *
 * 每槽只说一件事、只有一种皮。此前同一位置在五扇窗承载四种语义、穿四套皮
 * （语音放模式、字幕放运行状态、对话放会话属性、录音放状态加计时）——「配置是什么」
 * 与「现在在做什么」是两件事，各归各槽之后，用户在任何一扇窗学会的读法在其余四扇
 * 都成立。
 */
export function HudHeader({
  icon,
  lead,
  title,
  context,
  status,
  uplink,
  lane,
  actions,
  children,
  className,
  ...rest
}: HudHeaderProps) {
  return (
    <header {...rest} className={className ? `luna-hud-header ${className}` : "luna-hud-header"}>
      <span className="luna-hud-title">
        {lead ?? icon}
        <strong className="luna-visually-hidden">{title}</strong>
      </span>
      {status ? <TaskStatus kind={status.kind} label={status.label} /> : null}
      {context ? <span className="luna-hud-context-badge">{context}</span> : null}
      {uplink?.leavesDevice ? (
        <HudUplinkDot name={uplink.name} active={uplink.active ?? true} />
      ) : null}
      {lane ? <div className="luna-hud-header-lane">{lane}</div> : null}
      {actions ? (
        <div className="luna-hud-header-actions" data-no-drag>
          {actions}
        </div>
      ) : null}
      {children}
    </header>
  );
}
