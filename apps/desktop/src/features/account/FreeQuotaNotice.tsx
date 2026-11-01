/**
 * 免费额度说明（纵切 3 C5，PRD-13-018 L1 / L2）——「订阅与用量」一段里的一块，不新开分段（ADR-121 ⑩）。
 *
 * 只在免费额度被扣下时出现，按成因各说一种下一步：
 * - `shared_device`：免费额度每台设备只发给一个账户；预付与订阅照常可用；判错了可以联系支持申诉，
 *   **申诉不要求提供任何额外的个人信息**（所以这里没有表单，也不收集任何东西）。
 * - `verification_required`：零扣费验证一次付款方式；按钮交给 Rust 打开验证页（地址不过 IPC，
 *   同结账），回到窗口时账户页整页重取（复用收单那一处的 focus 刷新）。
 *
 * 前端零业务（ADR-003）：成因由服务端判定、经权益 `free_quota` 下发；这里只按它渲染。锚点
 * `settings-section-free_quota` 是「领不到免费额度」失败卡的落点（`settings://account/free_quota`）。
 */

import { useTranslation } from "react-i18next";

import type { CloudError, FreeQuotaDto } from "../../ipc/cloud-bindings";
import { errorMessage } from "../../lib/cloud-error";

/** 深链尾段与本块锚点同一个名字（`settings://account/<它>`）。 */
export const FREE_QUOTA_SECTION = "free_quota";

/** 「验证付款方式」这一下的状态与动作（面板持有）。 */
export type FreeQuotaVerify = {
  pending: boolean;
  error: CloudError | null;
  /** 验证页已在浏览器里打开：说一句「完成后回来会自动刷新」。 */
  opened: boolean;
  onVerify: () => void;
};

export function FreeQuotaNotice({ freeQuota, verify }: { freeQuota: FreeQuotaDto | null; verify: FreeQuotaVerify }) {
  const { t } = useTranslation("cloud");
  if (freeQuota === null || freeQuota.state !== "withheld") return null;
  const sharedDevice = freeQuota.reason === "shared_device";
  return (
    <div
      id={`settings-section-${FREE_QUOTA_SECTION}`}
      className="luna-settings-panel__ledger"
      role="group"
      aria-label={t("usage.free_quota.title")}
      tabIndex={-1}
    >
      <h3 className="luna-settings-panel__ledger-title">{t("usage.free_quota.title")}</h3>
      <div className="cloud-account__standing" data-tone="warning" role="status">
        <div>
          <p className="cloud-account__standing-text">
            {t(sharedDevice ? "usage.free_quota.shared_device" : "usage.free_quota.verification_required")}
          </p>
          {sharedDevice ? (
            <p className="cloud-account__standing-text">{t("usage.free_quota.shared_device_appeal")}</p>
          ) : null}
        </div>
        {sharedDevice ? null : (
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            disabled={verify.pending}
            aria-busy={verify.pending}
            onClick={verify.onVerify}
          >
            {t("usage.free_quota.verify")}
          </button>
        )}
      </div>
      {verify.opened && !sharedDevice ? (
        <p className="luna-settings-panel__hint" role="status">
          {t("usage.free_quota.verify_opened")}
        </p>
      ) : null}
      {verify.error ? (
        <p className="luna-settings-panel__hint" role="status">
          {errorMessage(verify.error, t)}
        </p>
      ) : null}
    </div>
  );
}
