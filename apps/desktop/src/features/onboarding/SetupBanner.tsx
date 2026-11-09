// 「完成设置 (n/N)」（PRD-14-064）：引导收场后仍有未 done 项时呈现，点击以覆盖层打开
// 引导并落到第一个未完成的步骤；全部就绪自动消失。读的是与引导同一份 `setup_status`，
// 不自己判"做没做完"。
//
// 两种形态、同一个组件（PRD-07-050 / PRD-07-065）：工作区顶部的横幅（`banner`）与启动
// 概览里的一行（`row`，走跨 feature 的「下一步」行原语）。概览「有事才弹」的触发条件
// 之一就是这里有内容——而概览此前弹出来却只字不提装机，用户被打断了却拿不到被打断的
// 理由。两处同一份投影、同一套文案；概览在场时横幅不渲染，否则同一事实同屏两次。

import { useTranslation } from "react-i18next";
import { commands, type SetupStatusDto } from "../../ipc/bindings";
import { requestOnboarding } from "../../lib/app-events";
import { ListTodoIcon } from "@lunarca/editor-core/Icon";
import { projectSetupBanner } from "./use-setup-status";

export function SetupBanner({
  setup,
  variant = "banner",
  onDismiss,
}: {
  setup: SetupStatusDto | null;
  variant?: "banner" | "row";
  /** 概览形态：先关掉概览再拉引导覆盖层（覆盖层要压在工作区之上，不是概览之内）。 */
  onDismiss?: () => void;
}) {
  const { t } = useTranslation("onboarding");
  if (setup === null || !setup.completed) {
    return null;
  }
  const projection = projectSetupBanner(setup);
  if (projection.firstRemaining === null) {
    return null;
  }
  const counts = { done: projection.done, total: projection.total };
  const items = projection.remainingIds.map((id) => t(`step.${id}.title`)).join("、");
  const open = () => {
    onDismiss?.();
    requestOnboarding(projection.firstRemaining ?? undefined);
  };

  // 概览里它恒是栈顶（应用本身还没装好，谈别的都早），故恒为 accent。
  if (variant === "row") {
    return (
      <section className="luna-next-row" data-tone="accent" aria-label={t("banner.title", counts)}>
        <div className="luna-next-row__main">
          <ListTodoIcon size={16} />
          <p>
            {t("banner.title", counts)}
            <span className="luna-next-row__aside">{t("banner.body", { items })}</span>
          </p>
        </div>
        <div className="luna-next-row__actions">
          <button type="button" className="luna-btn luna-btn--primary" onClick={open}>
            {t("banner.action")}
          </button>
        </div>
      </section>
    );
  }

  return (
    <div className="luna-setup-banner" role="region" aria-label={t("banner.title", counts)}>
      <div className="luna-setup-banner__text">
        <strong>{t("banner.title", counts)}</strong>
        <span>{t("banner.body", { items })}</span>
      </div>
      <div className="luna-setup-banner__actions">
        {/* 关掉的是提醒，不是能力（设置里「重新运行引导」照常可达）。没有这颗键的话，
            明确决定不做某一项的人会被这条横幅跟一辈子——而一个永远收不掉、又没有关闭
            办法的横幅，最后被读成的是"这个应用坏了"。 */}
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => void commands.onboardingSetReminderDismissed(true)}
        >
          {t("banner.dismiss")}
        </button>
        <button type="button" className="luna-btn luna-btn--primary" onClick={open}>
          {t("banner.action")}
        </button>
      </div>
    </div>
  );
}
