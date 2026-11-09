/**
 * 选库（PRD-21-010 / 003）：列出云端库；默认库已被删掉、列表为空时给「新建笔记库」（ADR-178 ③），没有同步权益时给方案引导；
 * 高级数据保护的库（没有托管信封）列出但写明网页上不可访问。
 */

import { useTranslation } from "react-i18next";
import { messageFor } from "../../lib/error-message";
import type { VaultSummary } from "../../worker/protocol";

export type VaultListState =
  | { kind: "loading" }
  | { kind: "failed"; code: string }
  | { kind: "not_entitled" }
  | { kind: "ready"; vaults: VaultSummary[] };

export interface VaultPickerProps {
  state: VaultListState;
  onOpen(vault: VaultSummary): void;
  onRetry(): void;
  onCreate(): void;
}

export function VaultPicker({ state, onOpen, onRetry, onCreate }: VaultPickerProps) {
  const { t } = useTranslation("web");
  if (state.kind === "loading") return <p className="luna-web-hint">{t("vaults.loading")}</p>;
  if (state.kind === "failed") {
    return (
      <section className="luna-web-vaults">
        <p className="luna-web-error" role="alert">
          {messageFor(t, "error", state.code)}
        </p>
        <button type="button" className="luna-web-button" onClick={onRetry}>
          {t("common.retry")}
        </button>
      </section>
    );
  }
  if (state.kind === "not_entitled") {
    return (
      <section className="luna-web-vaults" data-testid="not-entitled">
        <h2 className="luna-web-vaults__title">{t("vaults.notEntitledTitle")}</h2>
        <p className="luna-web-hint">{t("vaults.notEntitled")}</p>
      </section>
    );
  }
  if (state.vaults.length === 0) {
    return (
      <section className="luna-web-vaults" data-testid="no-vaults">
        <p className="luna-web-hint">{t("vaults.empty")}</p>
        <button type="button" className="luna-web-button" onClick={onCreate}>
          {t("vaults.create")}
        </button>
      </section>
    );
  }
  return (
    <section className="luna-web-vaults" aria-labelledby="web-vaults-title">
      <h2 id="web-vaults-title" className="luna-web-vaults__title">
        {t("vaults.title")}
      </h2>
      <ul className="luna-web-list" data-testid="vault-list">
        {state.vaults.map((vault) => (
          <li key={vault.id}>
            {vault.locked_reason ? (
              <div className="luna-web-list__row luna-web-list__row--disabled" aria-disabled="true">
                <span className="luna-web-list__name">{vault.name ?? t("vaults.unnamed")}</span>
                <span className="luna-web-list__path">{messageFor(t, "error", vault.locked_reason)}</span>
              </div>
            ) : (
              <button type="button" className="luna-web-list__row" onClick={() => onOpen(vault)}>
                <span className="luna-web-list__name">{vault.name ?? t("vaults.unnamed")}</span>
              </button>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
