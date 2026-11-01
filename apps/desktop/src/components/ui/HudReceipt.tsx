import { useTranslation } from "react-i18next";
import { useFailureGuidance } from "../../hooks/use-failure-guidance";
import { goTo } from "../../lib/navigate";
import { CheckIcon, InfoIcon } from "./Icon";

export type HudReceiptAction = { label: string; onSelect: () => void };

export type HudReceiptValue =
  | {
      /** 刚做成了一件事：「已复制」「已存为速记」。停留一会儿自己淡出。 */
      kind: "success";
      message: string;
      /** 最多一个后续动作（「打开」「撤销」）——要更多就不是回执了。 */
      action?: HudReceiptAction;
    }
  | {
      /** 没做成：原因 + 一个修复动作，常驻到调用方清掉为止。 */
      kind: "error";
      message: string;
      /** 次级细节（底层原因），`aria-live` 关闭以免刷屏读屏。 */
      detail?: string | null;
      /**
       * 稳定错误码：没给 `action` 时，修复动作取 `i18n/failure.ts` 注册表的深链——与
       * `FailureCard` 同一张表，HUD 不另接一张。
       */
      code?: string | null;
      action?: HudReceiptAction;
    };

export interface HudReceiptProps {
  receipt: HudReceiptValue | null;
  /** 成功回执淡出完毕：调用方据此把它清掉（失败回执不会触发）。 */
  onExpire?: () => void;
}

/**
 * HUD 回执线（ADR-137 V5）——每扇悬浮窗底部的那一行，**反馈只在这里出现一次**。
 *
 * 此前每扇窗各有一套反馈：复制后按钮变字、状态行再说一遍；书签有自己的 toast；
 * 速记的「已存」、录音的「已保存」、便签的冲突各是一种皮。用户要学的读法有多少种，
 * 取决于他打开过几扇窗。收成一条之后：成功是一句会自己消失的短话（可带一个动作），
 * 失败是一条不消失的原因 + 一个修复动作。
 *
 * 停留时长是样式的事（`--hud-receipt-linger`，悬停或聚焦时暂停）：淡出动画播完即
 * `onExpire`，组件里不再养一个与样式各说各话的计时器。
 */
export function HudReceipt({ receipt, onExpire }: HudReceiptProps) {
  const { t } = useTranslation("common");
  // hook 必须先于提前返回：自带动作的失败与成功回执都不取修复动作（传 `null`）。
  const guidance = useFailureGuidance(
    receipt?.kind === "error" && !receipt.action ? (receipt.code ?? null) : null,
  );
  if (receipt === null) return null;

  const action: HudReceiptAction | undefined =
    receipt.action ??
    (guidance ? { label: t(guidance.labelKey), onSelect: () => goTo(guidance.route) } : undefined);

  return (
    <div
      className="luna-hud-receipt"
      data-kind={receipt.kind}
      data-no-drag
      role={receipt.kind === "error" ? "alert" : "status"}
      onAnimationEnd={(event) => {
        if (receipt.kind === "success" && event.target === event.currentTarget) onExpire?.();
      }}
    >
      <span className="luna-hud-receipt__icon" aria-hidden="true">
        {receipt.kind === "success" ? <CheckIcon size={14} /> : <InfoIcon size={14} />}
      </span>
      <span className="luna-hud-receipt__message">{receipt.message}</span>
      {receipt.kind === "error" && receipt.detail ? (
        <span className="luna-hud-receipt__detail" aria-live="off">
          {receipt.detail}
        </span>
      ) : null}
      {action ? (
        <button type="button" className="luna-hud-receipt__action" onClick={action.onSelect}>
          {action.label}
        </button>
      ) : null}
    </div>
  );
}
