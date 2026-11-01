/**
 * 登录撞上设备上限（纵切 3 C5，PRD-13-021）：「已达到设备上限（N 台）」+ 占着名额的设备 + 每台一颗
 * 「移除这台并登录」。
 *
 * 前端零业务（ADR-003）：移除与重发是 Rust 的**一条命令**（`account_sign_in_resolve_device_limit`）——
 * 撤销令牌与要重发的那次领取（口令从不在其中，ADR-158）只在 Rust 内存里，这里只交回选中的设备 id、按结果渲染。
 * 登录成功 / 回到等浏览器 / 又撞上限，账户状态都经 `account://changed` 推回来，本组件随之换掉或刷新。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, type CloudError, type DeviceLimitDto } from "../../ipc/cloud-bindings";
import { errorAction, errorText } from "../../lib/cloud-error";
import { formatInstant } from "../../lib/format-datetime";
import { refreshAccount } from "../../hooks/use-account";

/** 选择结束、本组件随账户状态离场时要留给登录流程的一句话。 */
export type DeviceLimitExit = { kind: "error"; error: CloudError };

/**
 * 这些失败之后挂着的选择已不在（令牌失效 / 配对过期 / 被取消）：账户状态回到未登录，本组件离场，
 * 原因交给登录流程去说。其余失败（连不上、限频、服务端出错）选择还在，就地说、可以再点一次。
 */
const EXITING_ERRORS: ReadonlySet<CloudError["code"]> = new Set([
  "device_limit_expired",
  "login_expired",
  "no_pending_login",
]);

export function DeviceLimitPicker({
  limit,
  onExit,
}: {
  limit: DeviceLimitDto;
  onExit: (exit: DeviceLimitExit) => void;
}) {
  const { t, i18n } = useTranslation("cloud");
  const [removing, setRemoving] = useState<string | null>(null);
  const [error, setError] = useState<CloudError | null>(null);
  const [again, setAgain] = useState(false);

  const remove = async (deviceId: string) => {
    setRemoving(deviceId);
    setError(null);
    setAgain(false);
    const result = await commands.accountSignInResolveDeviceLimit(deviceId);
    setRemoving(null);
    if (result.status === "error") {
      if (EXITING_ERRORS.has(result.error.code)) onExit({ kind: "error", error: result.error });
      else setError(result.error);
    } else if (result.data.outcome === "device_limit_reached") {
      setAgain(true);
    }
    void refreshAccount();
  };

  const platformLabel = (platform: string | null) =>
    platform ? t(`devices.platform.${platform}`, { defaultValue: "" }) : "";
  const expires = formatInstant(limit.expires_at, i18n.language);
  const action = error ? errorAction(error, t) : null;

  return (
    <section className="cloud-account__stack" aria-label={t("device_limit.title", { max: limit.max })}>
      <h3 className="luna-settings-panel__section-title">{t("device_limit.title", { max: limit.max })}</h3>
      <p className="luna-settings-panel__desc">{t("device_limit.desc", { max: limit.max })}</p>
      {again ? (
        <p className="luna-settings-panel__hint" role="status">
          {t("device_limit.again")}
        </p>
      ) : null}
      <ul className="luna-settings-panel__rows">
        {limit.devices.map((device) => (
          <li key={device.id} className="luna-settings-panel__row">
            <div className="luna-settings-panel__row-label">
              <span className="luna-settings-panel__row-title cloud-account__device-name">{device.name}</span>
              <p className="luna-settings-panel__row-desc">
                {[
                  platformLabel(device.platform),
                  device.last_seen_at
                    ? t("devices.last_seen", { time: formatInstant(device.last_seen_at, i18n.language) })
                    : t("devices.never_seen"),
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </p>
            </div>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              disabled={removing !== null}
              aria-busy={removing === device.id}
              onClick={() => void remove(device.id)}
            >
              {removing === device.id ? t("device_limit.removing") : t("device_limit.remove")}
            </button>
          </li>
        ))}
      </ul>
      <p className="luna-settings-panel__hint">{t("device_limit.note")}</p>
      {expires ? <p className="luna-settings-panel__hint">{t("device_limit.expires", { time: expires })}</p> : null}
      {error ? (
        <div role="alert">
          <p className="luna-field-error">{errorText(error, t)}</p>
          {action ? <p className="luna-settings-panel__hint">{action}</p> : null}
        </div>
      ) : null}
      <div className="luna-settings-panel__actions">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={removing !== null}
          onClick={() => {
            void commands.accountSignInCancel().then(() => refreshAccount());
          }}
        >
          {t("device_limit.cancel")}
        </button>
      </div>
    </section>
  );
}
