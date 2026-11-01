/**
 * 登录状态下改密码（ADR-158 ⑤）：密码只在应用里输，服务端网页不再有任何密码框。
 *
 * 前端零业务（ADR-003）：注册新的 OPAQUE 记录、换回的令牌写钥匙链都在 Rust（`account_change_password`）；
 * 这里只核两次输入一致、发命令、说结果。这台设备保持登录，其余设备退出。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, type CloudError } from "../../ipc/cloud-bindings";
import { errorAction, errorText } from "../../lib/cloud-error";

export function ChangePassword() {
  const { t } = useTranslation("cloud");
  const [open, setOpen] = useState(false);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [mismatch, setMismatch] = useState(false);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<CloudError | null>(null);

  const close = () => {
    setOpen(false);
    setPassword("");
    setConfirm("");
    setMismatch(false);
    setError(null);
  };

  if (!open) {
    return (
      <div className="cloud-account__stack">
        {done ? (
          <p className="luna-settings-panel__hint" role="status">
            {t("password.changed")}
          </p>
        ) : null}
        <div className="luna-settings-panel__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => {
              setDone(false);
              setOpen(true);
            }}
          >
            {t("password.change_open")}
          </button>
        </div>
      </div>
    );
  }

  const submit = async () => {
    if (busy) return;
    setError(null);
    if (password !== confirm) {
      setMismatch(true);
      return;
    }
    setMismatch(false);
    setBusy(true);
    const result = await commands.accountChangePassword(password);
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    close();
    setDone(true);
  };

  const action = error ? errorAction(error, t) : null;
  return (
    <form
      className="cloud-account__stack"
      aria-label={t("password.change_open")}
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <p className="luna-settings-panel__desc">{t("password.change_desc")}</p>
      <label className="cloud-account__field">
        <span>{t("password.new_password")}</span>
        <input
          type="password"
          className="luna-input"
          autoComplete="new-password"
          value={password}
          disabled={busy}
          onChange={(event) => setPassword(event.currentTarget.value)}
        />
      </label>
      <label className="cloud-account__field">
        <span>{t("password.confirm_password")}</span>
        <input
          type="password"
          className="luna-input"
          autoComplete="new-password"
          value={confirm}
          disabled={busy}
          onChange={(event) => setConfirm(event.currentTarget.value)}
        />
      </label>
      <p className="luna-settings-panel__hint">{t("password.rule")}</p>
      {mismatch ? (
        <p className="luna-field-error" role="alert">
          {t("password.mismatch")}
        </p>
      ) : null}
      {error ? (
        <div role="alert">
          <p className="luna-field-error">{errorText(error, t)}</p>
          {action ? <p className="luna-settings-panel__hint">{action}</p> : null}
        </div>
      ) : null}
      <div className="luna-settings-panel__actions">
        <button type="submit" className="luna-btn luna-btn--primary" disabled={busy || password === ""}>
          {busy ? t("password.working") : t("password.change_submit")}
        </button>
        <button type="button" className="luna-btn luna-btn--secondary" disabled={busy} onClick={close}>
          {t("code.back")}
        </button>
      </div>
    </form>
  );
}
