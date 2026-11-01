/**
 * 服务器身份 —— 「这台服务器是谁」在界面上的唯一一处（账户页「高级」折叠 + 登录中的变更确认）。
 *
 * 协议裁决是 ADR-060（主路径 = 组织配置文件 / 邀请链接；副路径 = 首连钉住两把**签名公钥**），
 * 落地裁决是 ADR-071。统一登录之后（ADR-155）身份核验在登录流程里自动完成，钉住值就是生效的
 * 验签公钥；公钥与指纹只在用户主动要看、或服务器换了钥时出现。
 *
 * # 前端零业务（ADR-003）
 *
 * 未钉住 / 已钉住 / 变更阻断 / env 旁路 / 官方云这五档由 Rust 算好（`TrustStatusDto.state`），本组件
 * **只按它渲染**，不拿 `pinned` 与 `offered` 自己再推一遍。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, type TrustImportDto } from "../../ipc/bindings";
import {
  commands as cloudCommands,
  type AnchorKeysDto,
  type CloudError,
  type PinnedAnchorDto,
  type TrustStatusDto,
} from "../../ipc/cloud-bindings";
import { formatDateTime } from "../../lib/format-datetime";
import { isDialogCancelled } from "../../lib/vault-error";
import { errorMessage } from "../../lib/cloud-error";
import { refreshAccount, useAccount } from "../../hooks/use-account";

/** 导入的即时反馈（根证书去向 / 出错原因）。状态本身仍以 `TrustStatusDto` 为准。 */
type ImportFeedback =
  | { kind: "none" }
  | { kind: "busy" }
  | { kind: "done"; result: TrustImportDto }
  | { kind: "failed"; message: string };

/** 后端错误串的稳定前缀 → i18n key（前端只认 code）。 */
function importErrorKey(message: string): string {
  const code = message.split(":")[0]?.trim() ?? "";
  switch (code) {
    case "TRUST_FILE_UNREADABLE":
    case "TRUST_FILE_MALFORMED":
    case "TRUST_FILE_NOT_ORG_CONFIG":
    case "TRUST_FILE_UNSUPPORTED_VERSION":
    case "TRUST_FILE_BAD_SERVER_URL":
    case "TRUST_FILE_BAD_PUBKEY":
    case "TRUST_CREDENTIAL_CLEAR_FAILED":
    case "TRUST_CLOUD_UNAVAILABLE":
      return `trust.import_error.${code}`;
    default:
      // 取消对话框之外的未知码：如实展示原串，而不是套一句「导入失败」把唯一的线索盖掉。
      return "";
  }
}

/** 两把公钥的完整 hex（已由 Rust 按 4 字符分组，前端**不再加工**）。 */
function KeyPair({ keys, label }: { keys: AnchorKeysDto; label: string }) {
  const { t } = useTranslation("settings");
  return (
    <div className="luna-trust__keys">
      <p className="luna-trust__keys-label">{label}</p>
      <p className="luna-trust__keys-row">
        <span className="luna-trust__keys-name">{t("trust.key_lease")}</span>
        {/* 完整 64 位 hex，**不是短指纹**：服务端启动日志打印的就是这一串，用户要做的正是把
            两边逐字比一遍。 */}
        <code className="luna-trust__hex">{keys.lease_pubkey}</code>
      </p>
      <p className="luna-trust__keys-row">
        <span className="luna-trust__keys-name">{t("trust.key_catalog")}</span>
        <code className="luna-trust__hex">{keys.catalog_pubkey}</code>
      </p>
      {/* 部署标识是**信息性**参考：它可以合法变更，不参与钉住判定（钥才是身份）。 */}
      <p className="luna-trust__deployment">{t("trust.deployment_id", { id: keys.deployment_id })}</p>
    </div>
  );
}

function pinnedLabel(
  pinned: PinnedAnchorDto,
  t: (key: string, options?: Record<string, unknown>) => string,
  locale: string,
) {
  return {
    // locale 由界面语言注入（TDD-11 §1.4b）。
    when: formatDateTime(pinned.pinned_at * 1000, locale),
    how: t(`trust.provenance.${pinned.provenance}`),
  };
}

/**
 * 服务器换了钥：新旧并示，要求一次显式确认（ADR-060 ②）。确认之前验签一律继续用旧钉住钥
 * （Rust 侧保证）。登录流程撞上它、账户页看见它，用的都是这一块。
 */
export function TrustChangeConfirm({
  status,
  onConfirmed,
  onCancel,
}: {
  status: TrustStatusDto;
  onConfirmed: () => void;
  onCancel?: () => void;
}) {
  const { t, i18n } = useTranslation("settings");
  const { t: tc } = useTranslation("cloud");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<CloudError | null>(null);
  const confirm = async () => {
    setBusy(true);
    setError(null);
    const result = await cloudCommands.trustConfirmChange();
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    void refreshAccount();
    onConfirmed();
  };
  return (
    <div className="luna-trust__blocked" role="alert">
      <p className="luna-trust__blocked-title">{t("trust.changed_title")}</p>
      <p className="luna-trust__blocked-body">{t("trust.changed_body")}</p>
      {status.pinned ? (
        <KeyPair keys={status.pinned} label={t("trust.pinned_label", pinnedLabel(status.pinned, t, i18n.language))} />
      ) : null}
      {status.offered ? <KeyPair keys={status.offered} label={t("trust.offered_label")} /> : null}
      {/* 重钉的后果写在按钮上方：用户在点之前有权知道自己会被登出、目录会从头开始。 */}
      <p className="luna-trust__blocked-body">{t("trust.repin_consequences")}</p>
      <div className="luna-trust__actions">
        <button type="button" className="luna-btn luna-btn--secondary" disabled={busy} onClick={() => void confirm()}>
          {t("trust.confirm_change")}
        </button>
        {onCancel ? (
          <button type="button" className="luna-btn luna-btn--secondary" disabled={busy} onClick={onCancel}>
            {t("trust.cancel")}
          </button>
        ) : null}
      </div>
      {error ? (
        <p className="luna-field-error" role="alert">
          {errorMessage(error, tc)}
        </p>
      ) : null}
    </div>
  );
}

/** 账户页「高级」折叠里的那一块：这台服务器的身份 + 导入组织配置文件。 */
export function ServerIdentity() {
  const { t, i18n } = useTranslation("settings");
  const account = useAccount();
  const [status, setStatus] = useState<TrustStatusDto | null>(null);
  const [feedback, setFeedback] = useState<ImportFeedback>({ kind: "none" });

  const refresh = useCallback(async () => {
    const result = await cloudCommands.trustStatus();
    if (result.status === "ok") setStatus(result.data);
  }, []);

  // 账户变了（换服务器 / 登录 / 重钉）→ 身份结论跟着重读。
  useEffect(() => {
    void refresh();
  }, [refresh, account]);

  const doImport = useCallback(async () => {
    setFeedback({ kind: "busy" });
    const result = await commands.trustImportOrgConfig();
    if (result.status === "ok") {
      setFeedback({ kind: "done", result: result.data });
    } else if (isDialogCancelled(result.error)) {
      // 在文件选择器上按取消是用户的意思表示，不是错误（真机 2026-09-11）。
      setFeedback({ kind: "none" });
    } else {
      setFeedback({ kind: "failed", message: result.error });
    }
    await refresh();
  }, [refresh]);

  if (!status) return null;

  return (
    <section className="luna-trust" aria-label={t("trust.title")}>
      {status.state === "changed" ? (
        <TrustChangeConfirm status={status} onConfirmed={() => void refresh()} />
      ) : null}
      {status.state === "pinned" && status.pinned ? (
        <>
          <p className="luna-trust__ok" role="status">
            {t("trust.pinned_summary", pinnedLabel(status.pinned, t, i18n.language))}
          </p>
          <details className="luna-trust__details">
            <summary>{t("trust.show_keys")}</summary>
            <KeyPair
              keys={status.pinned}
              label={t("trust.pinned_label", pinnedLabel(status.pinned, t, i18n.language))}
            />
          </details>
        </>
      ) : null}
      {/* env 旁路（仅调试构建）：钉住整体不参与判定，如实说出来。 */}
      {status.state === "bypassed" ? <p className="luna-settings-panel__hint">{t("trust.bypassed")}</p> : null}
      {status.state === "unpinned" ? <p className="luna-settings-panel__hint">{t("trust.unpinned")}</p> : null}
      {status.state === "official" ? <p className="luna-settings-panel__hint">{t("trust.official")}</p> : null}
      {status.last_error ? (
        <p className="luna-settings-panel__hint">
          {t(`trust.probe_error.${status.last_error}`, { defaultValue: t("trust.probe_error.unknown") })}
        </p>
      ) : null}

      <div className="luna-trust__actions">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={feedback.kind === "busy"}
          onClick={() => void doImport()}
        >
          {t("trust.import")}
        </button>
      </div>
      <p className="luna-settings-panel__hint">{t("trust.import_hint")}</p>

      {feedback.kind === "done" && feedback.result.outcome === "pinned" ? (
        <p className="luna-trust__ok" role="status">
          {t("trust.import_ok", { server: feedback.result.server_url })}{" "}
          {/* 根证书只对这台服务器生效，系统信任库不动（ADR-153）。 */}
          {feedback.result.tls_root.kind === "pinned"
            ? t("trust.tls_root_pinned")
            : feedback.result.tls_root.kind === "absent"
              ? t("trust.tls_root_absent")
              : t("trust.tls_root_invalid")}
        </p>
      ) : null}
      {feedback.kind === "done" && feedback.result.outcome === "needs_confirmation" ? (
        <p className="luna-trust__ok" role="status">
          {t("trust.import_needs_confirmation")}
        </p>
      ) : null}
      {feedback.kind === "failed" ? (
        <p className="luna-field-error" role="alert">
          {importErrorKey(feedback.message) ? t(importErrorKey(feedback.message)) : feedback.message}
        </p>
      ) : null}
    </section>
  );
}
