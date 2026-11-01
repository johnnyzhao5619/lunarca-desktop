/**
 * 账户数据权利（纵切 3 C6，PRD-13-020 / PRD-13-041）：「导出我的账户数据」与删除账户前的后果清单。
 *
 * - **导出**：按钮交给 Rust——Rust 弹保存对话框、把服务端的导出流式写进选中的位置，取消即什么都
 *   不发（`account_export`）。这里只渲染在途 / 已保存到哪里 / 失败原因三种状态。同一颗按钮也出现在
 *   删除确认里（「先导出」），状态只在点它的那一处说，不在页面上说两遍。
 * - **删除前的后果**：确认那一步先取 `account_deletion_preview`，只列适用的几条——在扣费的订阅会
 *   立即取消且当期不退、预付余额会作废（大于 0 时提醒 14 天内未用的流量包可在删除前找支持退款）、
 *   计费与用量记录的法定保留年限（不含邮箱）、本机笔记与录音不受影响。取不到时照常允许删除，
 *   只说没取到，**不编数字**。预览说「不能删」（协议 v17：组织里唯一的所有者、组织还有别的成员）时
 *   先说清下一步，确认按钮不给——能不能删是服务端的判定，这里只照着渲染。
 *
 * 前端零业务（ADR-003）：credits 已由 Rust 换算，保留年限、订阅档都是服务端给的事实。
 */

import { useTranslation } from "react-i18next";

import type {
  AccountDeletionPreviewDto,
  CloudError,
  DeletionBlockedReasonDto,
} from "../../ipc/cloud-bindings";
import { errorMessage } from "../../lib/cloud-error";
import { formatCount } from "../../lib/format-datetime";
import { serverLabel } from "./server-label";

/** 导出按钮出现在哪一处（状态只在点它的那一处说）。 */
export type ExportOrigin = "account" | "delete";

/** 导出的状态与动作（面板持有）。 */
export type AccountExportControl = {
  pending: boolean;
  /** 最近一次是从哪一处点的；`null` = 还没点过（或已清场）。 */
  origin: ExportOrigin | null;
  /** 写好的路径（用户自己选的位置）。 */
  savedPath: string | null;
  error: CloudError | null;
  onExport: (origin: ExportOrigin) => void;
};

/** 删除确认那一步的后果预览。 */
export type DeletionPreviewState =
  | { state: "loading" }
  | { state: "loaded"; preview: AccountDeletionPreviewDto }
  | { state: "failed"; error: CloudError };

/**
 * 预览说了「不能删」时的成因；在取、取不到、可以删都是 `null`（取不到照常允许删除，服务端仍会拦）。
 */
export function deletionBlockedReason(
  preview: DeletionPreviewState | null,
): DeletionBlockedReasonDto | null {
  if (preview?.state !== "loaded") return null;
  const { deletion } = preview.preview;
  return deletion.state === "blocked" ? deletion.reason : null;
}

/** 没有面板接线时（静态渲染）导出按钮不做事。 */
export const NO_EXPORT: AccountExportControl = {
  pending: false,
  origin: null,
  savedPath: null,
  error: null,
  onExport: () => undefined,
};

/**
 * 导出按钮 + 这一处的状态。`origin = "account"` 那一处带一句说明：导出里有什么、为什么没有笔记。
 */
export function AccountExport({ control, origin }: { control: AccountExportControl; origin: ExportOrigin }) {
  const { t } = useTranslation("cloud");
  const here = control.origin === origin;
  return (
    <div className="cloud-account__export">
      {origin === "account" ? <p className="luna-settings-panel__desc">{t("export.desc")}</p> : null}
      <div className="luna-settings-panel__actions">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={control.pending}
          aria-busy={control.pending && here}
          onClick={() => control.onExport(origin)}
        >
          {t(control.pending && here ? "export.pending" : "export.button")}
        </button>
      </div>
      {here && control.savedPath !== null ? (
        <p className="luna-settings-panel__hint cloud-account__saved" role="status">
          {/* 一句插值而不是「标签 + 路径」拼接：拼接在别的语序里会散架。 */}
          {t("export.saved", { path: control.savedPath })}
        </p>
      ) : null}
      {here && control.error !== null ? (
        <p className="luna-settings-panel__hint" role="status">
          {errorMessage(control.error, t)}
        </p>
      ) : null}
    </div>
  );
}

/**
 * 删除之后会怎样：只列适用的几条。取不到时说没取到并给出原因，不编数字——但不拦删除。
 */
export function DeletionConsequences({ preview }: { preview: DeletionPreviewState }) {
  const { t, i18n } = useTranslation("cloud");
  if (preview.state === "loading") {
    return (
      <p className="luna-settings-panel__hint" role="status">
        {t("delete.preview_loading")}
      </p>
    );
  }
  const local = <li>{t("delete.consequence_local")}</li>;
  if (preview.state === "failed") {
    return (
      <>
        <p className="luna-settings-panel__hint" role="status">
          {t("delete.preview_failed", { reason: errorMessage(preview.error, t) })}
        </p>
        <ul className="cloud-account__consequences">{local}</ul>
      </>
    );
  }
  const { subscription, prepaid_credits: credits, billing_retention_years: years } = preview.preview;
  const hasCredits = (credits ?? 0) > 0;
  const blocked = deletionBlockedReason(preview);
  return (
    <>
      {/* 不能删时先说下一步：后果照列，那是转让所有者之后再删时会发生的事。 */}
      {blocked === null ? null : (
        <p className="luna-settings-panel__hint" role="status">
          {t(`delete.blocked.${blocked}`)}
        </p>
      )}
      <ul className="cloud-account__consequences">
        {subscription.state === "active" ? (
          <li>
            {t("delete.consequence_subscription", {
              tier: serverLabel(
                (key) => t(key, { defaultValue: "" }),
                `tier.${subscription.tier}`,
                subscription.tier,
                t("tier.other"),
              ).label,
            })}
          </li>
        ) : null}
        {hasCredits ? (
          <li>{t("delete.consequence_credits", { credits: formatCount(credits, i18n.language) })}</li>
        ) : null}
        <li>{t("delete.consequence_retention", { count: years })}</li>
        {local}
      </ul>
    </>
  );
}
