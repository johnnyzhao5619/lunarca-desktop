/**
 * 「云同步」一节里这台设备的同步偏好（ADR-148 ⑩、ADR-156）：录音同步方式、按流量计费网络与
 * 电量低时暂停大文件。
 *
 * 键、默认值、选项与控件形态都在 Rust 设置注册表（`sync.*`，由这一节承载、不渲染成设置行）；
 * 这里按注册表声明的形态渲染，不按键名猜。前端零业务：改动直接写设置，同步服务每轮读取。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { SelectBox } from "../../components/ui/SelectBox";
import { commands, events, type SettingSpecDto } from "../../ipc/bindings";

/** 这一节承载的键，按显示顺序。 */
const SYNC_PREFERENCE_KEYS = [
  "sync.recordings_mode",
  "sync.pause_large_on_metered",
  "sync.pause_large_on_battery",
] as const;

function useSyncPreferenceSpecs(): SettingSpecDto[] {
  const [specs, setSpecs] = useState<SettingSpecDto[]>([]);
  useEffect(() => {
    let disposed = false;
    const load = () => {
      void commands.listSettings().then((result) => {
        if (disposed || result.status !== "ok") return;
        setSpecs(
          SYNC_PREFERENCE_KEYS.flatMap((key) =>
            result.data.filter((spec) => spec.key === key && spec.available),
          ),
        );
      });
    };
    load();
    const unlisten = events.settingsChanged.listen((event) => {
      if ((SYNC_PREFERENCE_KEYS as readonly string[]).includes(event.payload.key)) load();
    });
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);
  return specs;
}

export function SyncPreferences() {
  const { t } = useTranslation("settings");
  const specs = useSyncPreferenceSpecs();
  if (specs.length === 0) return null;
  return (
    <ul className="luna-settings-panel__rows">
      {specs.map((spec) => {
        const labelId = `cloud-sync-pref-${spec.key}`;
        const value = spec.current_value;
        return (
          <li key={spec.key} className="luna-settings-panel__row" data-setting-key={spec.key}>
            <div className="luna-settings-panel__row-label">
              <span id={labelId} className="luna-settings-panel__row-title">
                {t(spec.i18n_key)}
              </span>
              <span className="luna-settings-panel__row-desc">{t(spec.desc_i18n_key)}</span>
            </div>
            {spec.control.kind === "select" && value.kind === "str" ? (
              <SelectBox
                labelledBy={labelId}
                value={value.value}
                onChange={(event) =>
                  void commands.setSetting(spec.key, { kind: "str", value: event.target.value })
                }
              >
                {spec.control.options.map((option) => (
                  <option key={option} value={option}>
                    {t(`option.${spec.key}.${option}`)}
                  </option>
                ))}
              </SelectBox>
            ) : null}
            {spec.control.kind === "toggle" && value.kind === "bool" ? (
              <button
                type="button"
                role="switch"
                aria-checked={value.value}
                aria-labelledby={labelId}
                className="luna-switch"
                onClick={() => void commands.setSetting(spec.key, { kind: "bool", value: !value.value })}
              >
                <span className="luna-switch__track" aria-hidden="true">
                  <span className="luna-switch__thumb" />
                </span>
              </button>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
