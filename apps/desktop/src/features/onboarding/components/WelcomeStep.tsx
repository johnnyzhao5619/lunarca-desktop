// 欢迎：语言与外观（PRD-14-002）+ 隐私承诺（PRD-14-001，「数据留在本地、算力可选」）。
// 两组都走引导卡原语：此前语言与外观是两行无边框裸控件、承诺是三张卡，于是同一页里
// **可操作的那两行反而没有可点的形状**，读起来像页头的附属说明。
// 写的是与设置页同一对键（appearance.language / appearance.theme），改完即生效
// （settings://changed → useAppearance 重投影），不另起状态。
//
// 取数与写入走 `useSettings()`——与设置页同一条路。此前这里手写了一份 `listSettings()`
// + `setSetting()` + 手动派发 `SETTINGS_CHANGED_EVENT`，也就是把那个 hook 又实现了一遍，
// 而它的错误约定与设置页并不一致。

import { useTranslation } from "react-i18next";
import type { SettingSpecDto } from "../../../ipc/bindings";
import { CpuIcon, GlobeIcon, ShieldIcon } from "../../../components/ui/Icon";
import { useSettings } from "../../settings/use-settings";
import { strValue } from "../../settings/SettingControls";

const PROMISES = [
  { key: "local", Icon: CpuIcon },
  { key: "compute", Icon: ShieldIcon },
  { key: "network", Icon: GlobeIcon },
] as const;

/** 首屏就能改的两项；选项集合来自 Rust 注册表（`control.options`），前端不自列。 */
const QUICK_KEYS = ["appearance.language", "appearance.theme"] as const;

function optionsOf(spec: SettingSpecDto | undefined): string[] {
  return spec?.control.kind === "select" ? spec.control.options : [];
}

export function WelcomeStep() {
  const { t } = useTranslation("onboarding");
  const { t: ts } = useTranslation("settings");
  const { specs, setValue, error } = useSettings();

  return (
    <div className="luna-onboarding-step">
      <div className="luna-onboarding-list">
        {QUICK_KEYS.map((key) => {
          const spec = specs.find((item) => item.key === key);
          const current = strValue(spec);
          return (
            <div key={key} className="luna-onboarding-item" data-layout="columns">
              <span className="luna-onboarding-item__label">{ts(key)}</span>
              <div className="luna-segmented-control" role="radiogroup" aria-label={ts(key)}>
                {optionsOf(spec).map((option) => (
                  <button
                    key={option}
                    type="button"
                    role="radio"
                    aria-checked={option === current}
                    className="luna-segmented-control__item"
                    data-active={option === current}
                    onClick={() => void setValue(key, { kind: "str", value: option })}
                  >
                    {ts(`option.${key}.${option}`)}
                  </button>
                ))}
              </div>
            </div>
          );
        })}
      </div>
      {error ? (
        <p className="luna-onboarding__error" role="alert">
          {error}
        </p>
      ) : null}

      <ul className="luna-onboarding-list">
        {PROMISES.map(({ key, Icon }) => (
          <li key={key} className="luna-onboarding-item">
            <span className="luna-onboarding-item__icon" aria-hidden="true">
              <Icon size={18} />
            </span>
            <div className="luna-onboarding-item__text">
              <strong>{t(`step.welcome.promise.${key}.title`)}</strong>
              <p>{t(`step.welcome.promise.${key}.desc`)}</p>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
