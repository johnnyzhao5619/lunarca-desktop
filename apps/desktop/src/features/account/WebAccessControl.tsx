import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useConfirm } from "../../components/ConfirmHost";
import { commands, type CloudError } from "../../ipc/cloud-bindings";
import { errorText } from "../../lib/cloud-error";
import { WebSessions } from "./WebSessions";

/** 账户级网页访问开关（PRD-21-006）。 */
export function WebAccessControl() {
  const { t } = useTranslation("cloud");
  const ask = useConfirm();
  const mounted = useRef(false);
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<CloudError | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    const result = await commands.accountWebAccess();
    if (!mounted.current) return;
    if (result.status === "ok") setEnabled(result.data);
    else setError(result.error);
    setBusy(false);
  }, []);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => { mounted.current = false; };
  }, [load]);

  async function toggle() {
    if (busy || enabled === null) return;
    setBusy(true);
    if (enabled && !await ask({
      title: t("web_access.disable_title"),
      description: t("web_access.disable_description"),
      confirmLabel: t("web_access.disable_confirm"),
    })) {
      setBusy(false);
      return;
    }
    if (!mounted.current) return;
    setError(null);
    const result = await commands.accountSetWebAccess(!enabled);
    if (!mounted.current) return;
    if (result.status === "ok") setEnabled(result.data);
    else setError(result.error);
    setBusy(false);
  }

  return (
    <div>
      <div className="luna-settings-panel__row" aria-busy={busy}>
        <span className="luna-settings-panel__row-title">{t("web_access.label")}</span>
        {enabled !== null ? (
          <button
            type="button"
            role="switch"
            className="luna-switch"
            aria-label={t("web_access.label")}
            aria-checked={enabled}
            disabled={busy}
            onClick={() => void toggle()}
          >
            <span className="luna-switch__track" aria-hidden="true">
              <span className="luna-switch__thumb" />
            </span>
          </button>
        ) : error ? (
          <button type="button" className="luna-btn luna-btn--secondary" onClick={() => void load()}>
            {t("web_access.retry")}
          </button>
        ) : <span role="status">{t("web_access.loading")}</span>}
      </div>
      {error ? <p className="luna-field-error" role="alert">{errorText(error, t)}</p> : null}
      {enabled ? <WebSessions /> : null}
    </div>
  );
}
