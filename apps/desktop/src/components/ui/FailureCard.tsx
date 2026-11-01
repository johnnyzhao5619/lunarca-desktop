import { useTranslation } from "react-i18next";
import { failureMessage } from "../../i18n/failure";
import { useFailureGuidance } from "../../hooks/use-failure-guidance";
import { goTo } from "../../lib/navigate";

export interface FailureCardProps {
  /** 稳定错误码（`JobError::code()`）。成因句与修复深链都由它驱动。 */
  code: string | null;
  /** 覆盖注册表成因句（事件里已算好本地化消息的表面——翻译槽——直接传）。 */
  message?: string;
  /**
   * 后果从句——**表面差异的唯一容身处**（ADR-111 ④）：同一个码在对话里是
   * 「这一轮没有开始」、在录音里是「录音继续，停录后补齐」。成因句永远只有注册表一份。
   */
  consequence?: string;
  /** 次级细节行（如「最短等待 N 秒」），`aria-live` 关闭以免倒计时刷屏读屏。 */
  detail?: string;
  /** 重试动作（可退避禁用）；文案由表面给（重试 / 重新生成 / N 秒后重试）。 */
  retry?: { label: string; disabled?: boolean; onClick: () => void };
}

/**
 * 失败卡（`.luna-failure-card`）——错误文本的第三种形态，与 `.luna-field-error`（行内
 * 一句红字）、`.luna-error-banner`（整块横幅）并列：**带修复动作的失败**。成因句 +
 * 可选后果从句 + 修复深链（注册表驱动）+ 可选重试。
 *
 * 此前对话与翻译弹窗各画一张几乎相同的红卡（`.luna-chat-failure` /
 * `.luna-popup-slot__alert`），各接一张深链表——两张表已经漂了（QUOTA_EXHAUSTED
 * 的账户页深链只有 chat 知道）。收敛后修复动作只从 `i18n/failure.ts` 一张表来。
 */
export function FailureCard({ code, message, consequence, detail, retry }: FailureCardProps) {
  const { t } = useTranslation("common");
  const guidance = useFailureGuidance(code);
  return (
    <div className="luna-failure-card" role="alert">
      <p className="luna-failure-card__message">{message ?? failureMessage(code)}</p>
      {consequence ? <p className="luna-failure-card__message">{consequence}</p> : null}
      {detail ? (
        <p className="luna-failure-card__detail" aria-live="off">
          {detail}
        </p>
      ) : null}
      {guidance || retry ? (
        <div className="luna-failure-card__actions">
          {guidance ? (
            <button
              type="button"
              className="luna-failure-card__action"
              onClick={() => goTo(guidance.route)}
            >
              {t(guidance.labelKey)}
            </button>
          ) : null}
          {retry ? (
            <button
              type="button"
              className="luna-failure-card__action"
              data-variant="primary"
              disabled={retry.disabled}
              onClick={retry.onClick}
            >
              {retry.label}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
