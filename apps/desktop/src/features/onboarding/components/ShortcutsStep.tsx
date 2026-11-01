/**
 * 引导的**「快捷键」一步**：就地改三个全局键（PRD-14-052 收尾）。
 *
 * 这一步此前只把绑定**读出来给用户看**，改键得等进了应用再去设置里找。而这正是用户最
 * 想改它们的时刻：他刚知道这三个键是什么、也刚发现其中一个和自己习惯的组合撞了。
 *
 * **自成一步而不是挂在「第一个任务」里**（2026-09-10 真机）：它此前被那一步渲染**两次**
 * （完成态一次、未完成态一次），于是同一块内容有两个位置；而「第一个任务」本身要讲的是
 * "就地试一次"，再叠三个可录入的键盘控件，一页里同时要求你"试"和"改"两件事。拆开之后
 * 任务那一步只留任务卡（键名仍在卡片文案里就地报出，不必翻回来看）。
 *
 * 录制器、冲突判定、双击开关、系统保留键的拒绝、行级错误与「已保存」反馈全部复用设置页
 * 那一套（`useSettingsController` + `SettingControl` 按注册表下发的 `control.kind` 分发）
 * ——引导另写一份录制器，就会有两处对"这个键能不能录"给出不同答案。这里只挑三行、
 * 换个排版。
 */

import { useTranslation } from "react-i18next";
import { goTo } from "../../../lib/navigate";
import { HEADLINE_SHORTCUT_KEYS } from "../../../lib/shortcut-display";
import { SettingControl } from "../../settings/SettingControls";
import { settingErrorText } from "../../settings/settings-errors";
import { useSettingsController } from "../../settings/use-settings-controller";

export function ShortcutsStep() {
  const { t } = useTranslation("onboarding");
  const { t: ts } = useTranslation("settings");
  const controller = useSettingsController(true);

  const rows = HEADLINE_SHORTCUT_KEYS.map((key) =>
    controller.specs.find((spec) => spec.key === key),
  ).filter((spec) => spec !== undefined);

  // 设置还没读回来时整块不渲染，而不是先摆三行空槽再填。
  if (rows.length === 0) {
    return null;
  }

  // 标题与引子由引导壳层统一渲染（`step.shortcuts.title` / `.body`）——每一步各写一份
  // 标题，就会有一步的字号或间距和别的步不一样。
  return (
    <section className="luna-onboarding-step">
      <div className="luna-onboarding-list">
        {rows.map((spec) => {
          const failure = controller.errors[spec.key];
          return (
            <div key={spec.key} className="luna-onboarding-item" data-layout="columns">
              <span className="luna-onboarding-item__label">{ts(spec.i18n_key)}</span>
              <SettingControl
                spec={spec}
                setValue={controller.setValue}
                invalid={failure !== undefined}
                clearError={() => controller.clearError(spec.key)}
                onVaultChanged={() => {
                  // 快捷键项与 Vault 无关；这个回调是控件族的通用签名。
                }}
              />
              {/* 写不进去是**行级**信息（是哪个组合被拒、为什么），措辞与设置页同一处。 */}
              {failure ? (
                <p className="luna-onboarding__error" role="alert">
                  {settingErrorText(failure, ts)}
                </p>
              ) : null}
            </div>
          );
        })}
      </div>
      {/* 次要按钮而不是下划线链接：这一步唯一的其它出口，与各步的「重新检测」「去准备模型」
          同一形制；裸链接在一列卡片下面读起来像正文里漏出来的一句。 */}
      <button
        type="button"
        className="luna-btn luna-btn--secondary"
        onClick={() => goTo("settings://shortcuts")}
      >
        {t("shortcuts.all")}
      </button>
    </section>
  );
}
