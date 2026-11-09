// 首启引导向导外壳（PRD-14 §3 / ADR-114）。步骤顺序与三态由 Rust `setup_status` 下发，
// 这里**按下发的顺序**渲染（`controller.order`，不是前端那份常量——此前这句话是假的，
// 而假在哪里不会以任何形式现形）；每步内容为独立组件（components/），检测/推荐/权限/绑定
// 等业务全在 Rust，前端只发命令、订阅、渲染（ADR-003）。
//
// 壳层承担的交互契约：
// - 步骤指示器三态（未到访 / 跳过 / 已完成），任一步可点回；窄卡退化为「第 N 步 / 共 N」+ 进度条；
// - Vault 绑定后每一步常驻「先进入应用，稍后完成」（PRD-14-041）；
// - 切步后焦点落到标题、内容区轻过渡（尊重 reduce-motion）；Enter 触发主动作（焦点不在输入控件时）；
// - 模型下载转入后台后，底部常驻一枚进度胶囊，切步不丢（PRD-14-020）；
// - **主按钮灰着时必须说出原因，并给一条出路**（`Blocker`）：步骤指示器允许直接跳到
//   任一步，于是用户可以在没绑 Vault 时站到最后一步——此前「开始使用」就那么灰着，
//   界面上没有一个字解释，「上一步」是唯一出口。原因行归壳层统一渲染，各步只上报。

import { useEffect, useMemo, useRef, type KeyboardEvent } from "react";
import { commands } from "../../ipc/bindings";
import { useTranslation } from "react-i18next";
import { canFinish, type OnboardingStepId } from "./onboarding-steps";
import type { OnboardingController } from "./use-onboarding";
import { useHardwareSummary } from "./use-hardware";
import { useModels } from "../settings/use-settings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { cssVariables } from "../../lib/css-variables";
import { LunaLogo } from "../../components/ui/LunaLogo";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { parseProgressId } from "../settings/CoremlEncoderRow";
import { WelcomeStep } from "./components/WelcomeStep";
import { ComputeStep } from "./components/ComputeStep";
import { ModelsStep } from "./components/ModelsStep";
import { PermissionsStep } from "./components/PermissionsStep";
import { VaultStep } from "./components/VaultStep";
import { FirstTaskStep } from "./components/FirstTaskStep";
import { ShortcutsStep } from "./components/ShortcutsStep";

type OnboardingWizardProps = {
  controller: OnboardingController;
  /** 以覆盖层呈现（「重新运行引导」/「完成设置」横幅进来的重入，PRD-14-060）；首启为全屏。 */
  overlay?: boolean;
  /** 覆盖层的关闭回调（Esc / 「关闭」）。 */
  onClose?: () => void;
};

/** 焦点落在这些控件上时 Enter 归它们自己，不触发向导主动作。 */
const ENTER_OWNED_BY = new Set(["INPUT", "TEXTAREA", "SELECT", "BUTTON", "A"]);

/**
 * 主按钮为什么不能按 + 去哪解决。步骤把它交给壳层，壳层统一渲染成原因行。
 *
 * `fix` 只在**解决它要离开本步**时给（如"去绑 Vault"）；要填的东西就在同屏时不给按钮
 * ——那只会多一颗指向用户正看着的表单的键。
 */
export type Blocker = { reason: string; fixLabel?: string; fix?: () => void };

export function OnboardingWizard({ controller, overlay = false, onClose }: OnboardingWizardProps) {
  const { t, i18n } = useTranslation("onboarding");
  const { current, setup, states, order, markStep, goNext, goTo, finish } = controller;
  // 硬件摘要与模型清单在壳层各取一次：模型准备页与底部进度胶囊共享同一份进度。
  const hardware = useHardwareSummary();
  const models = useModels();
  const headingRef = useRef<HTMLHeadingElement>(null);
  // 覆盖层形态是模态：Tab 圈在卡内、Esc 关闭、关闭后焦点归还到打开它的那颗按钮
  // （横幅 / 设置行）。首启全屏形态下面没有别的界面，不需要圈。
  const dialogRef = useDialogBehavior<HTMLElement>({
    onClose: () => onClose?.(),
    enabled: overlay,
    initialFocusRef: headingRef,
    modal: true,
  });

  const index = order.indexOf(current);
  const step: OnboardingStepId = order[index] ?? "welcome";
  const isLast = index === order.length - 1;
  const prev: OnboardingStepId | null = index > 0 ? order[index - 1] : null;
  const finishable = canFinish(states);

  // 切步：焦点落到标题（读屏用户听到新一步的名字，键盘用户从头开始 Tab）。
  useEffect(() => {
    headingRef.current?.focus();
  }, [step]);

  // 本机漏斗记录（PRD-14 §1）：记在**壳层**而不是编排 hook 里——那个 hook 在主窗恒挂载，
  // 且 `current` 对已装机用户也默认是 "welcome"，挂在那里会给每次启动都记一条根本没发生过
  // 的「进了欢迎页」。壳层只在引导真的显示时才存在，它看到的切步就是真的切步。
  useEffect(() => {
    void commands.onboardingFunnelEnter(step);
  }, [step]);

  /**
   * 后台进行中的那一条（任一条未完成即显示胶囊，PRD-14-020）。
   *
   * 只报**认得出名字**的那些：进度表里的 id 是安装键，不都是模型 id（CoreML 那条是
   * `<id>#coreml`，随包档的键也不在目录里）。此前按原样查不到就把 id 当名字显示，
   * 于是胶囊上出现的是 `whisper-tiny-bundled` 这种内部标识（真机 A2）。
   *
   * CoreML 那条有自己的行内进度与阶段文案（`CoremlEncoderRow`），胶囊只说它在准备加速，
   * 不重复"下载了多少字节"——那一段此刻多半根本不是在下载，而是在解压或编译。
   */
  const activeDownload = useMemo(() => {
    for (const [id, progress] of Object.entries(models.progressById)) {
      if (progress.done) {
        continue;
      }
      const { modelId, coreml } = parseProgressId(id);
      const model = models.models.find((m) => m.id === modelId);
      if (!model) {
        continue;
      }
      if (coreml) {
        return { name: model.name, coreml: true, percent: null, done: 0, total: 0 };
      }
      const total = progress.total_bytes ?? 0;
      const downloaded = progress.downloaded_bytes ?? 0;
      // 收尾阶段（校验、落盘）字节数已满而尚未 `done`：还报一个"100%"会读成卡住。
      const ratio = total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
      return {
        name: model.name,
        coreml: false,
        percent: ratio !== null && ratio < 100 ? ratio : null,
        done: downloaded,
        total,
      };
    }
    return null;
  }, [models.progressById, models.models]);

  async function advance() {
    await markStep(step);
    goNext();
  }

  async function complete() {
    await markStep(step);
    await finish();
    onClose?.();
  }

  /** 主动作：最后一步 = 收场；Vault 未绑定 = 无（由 VaultStep 自己的按钮推进）；其余 = 下一步。 */
  const primary: { label: string; blocker: Blocker | null; run: () => void } | null = (() => {
    const goToVault: Blocker = {
      reason: t("blocked.needs_vault"),
      fixLabel: t("blocked.go_vault"),
      fix: () => goTo("vault"),
    };
    if (isLast) {
      return {
        // 覆盖层里的人**已经在用应用了**——对他说「开始使用」不成立。
        label: t(overlay ? "action.done" : "action.finish"),
        blocker: finishable ? null : goToVault,
        run: () => void complete(),
      };
    }
    if (step === "vault" && states.get("vault") !== "done") {
      return null;
    }
    return { label: t("action.next"), blocker: null, run: () => void advance() };
  })();

  function onCardKeyDown(event: KeyboardEvent<HTMLElement>) {
    // Esc 归 useDialogBehavior（覆盖层）；这里只管 Enter = 主动作。
    if (event.key !== "Enter" || event.metaKey || event.ctrlKey || event.altKey) {
      return;
    }
    const target = event.target as HTMLElement | null;
    if (!target || ENTER_OWNED_BY.has(target.tagName) || target.isContentEditable) {
      return;
    }
    if (primary && primary.blocker === null) {
      event.preventDefault();
      primary.run();
    }
  }

  function renderStepContent() {
    switch (step) {
      case "welcome":
        return <WelcomeStep />;
      case "vault":
        return (
          <VaultStep
            onBound={() => {
              void (async () => {
                await markStep("vault");
                goNext();
              })();
            }}
          />
        );
      case "compute":
        return <ComputeStep />;
      case "models":
        return <ModelsStep hardware={hardware} models={models} setup={setup} />;
      case "permissions":
        return <PermissionsStep setup={setup} onRefresh={() => void controller.refresh()} />;
      case "shortcuts":
        return <ShortcutsStep />;
      case "first_task":
        return (
          <FirstTaskStep
            setup={setup}
            onRefresh={() => void controller.refresh()}
            onGoToStep={goTo}
          />
        );
    }
  }

  return (
    <div className="luna-onboarding-viewport" data-overlay={overlay || undefined}>
      <main
        ref={dialogRef}
        className="luna-onboarding-card"
        role={overlay ? "dialog" : undefined}
        aria-modal={overlay || undefined}
        aria-label={t("title")}
        onKeyDown={onCardKeyDown}
      >
        <div className="luna-onboarding-card__top">
          <div className="luna-onboarding-logo" aria-hidden="true">
            <LunaLogo size={48} />
          </div>
          {overlay ? (
            <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
              {t("action.close")}
            </button>
          ) : null}
        </div>

        <nav className="luna-onboarding__stepper" aria-label={t("progress")}>
          {order.map((id, idx) => {
            const isCurrent = id === step;
            const state = states.get(id) ?? "pending";
            return (
              <button
                key={id}
                type="button"
                className="luna-onboarding__stepper-item"
                data-active={isCurrent}
                data-state={state}
                // `aria-disabled` 而不是 `disabled`：当前步这颗按钮确实不该有动作，但
                // `disabled` 会让它**不可聚焦**，于是键盘与读屏用户走过步进条时唯一读不到的
                // 恰好是「我在哪一步」。禁用的是动作，不是存在。
                aria-disabled={isCurrent || undefined}
                aria-current={isCurrent ? "step" : undefined}
                aria-label={t("stepper_label", {
                  title: t(`step.${id}.title`),
                  state: t(`state.${state}`),
                })}
                onClick={() => {
                  if (!isCurrent) {
                    goTo(id);
                  }
                }}
              >
                <span className="luna-onboarding__stepper-dot" aria-hidden="true">
                  {state === "done" ? "✓" : idx + 1}
                </span>
                <span className="luna-onboarding__stepper-label">{t(`step.${id}.title`)}</span>
              </button>
            );
          })}
          {/* 窄卡时的紧凑投影（CSS 容器查询二选一显示）：第 N 步 / 共 M + 进度条。
              **不对读屏隐藏**：它与上面那组带标签的步骤按钮由 `display:none` 天然互斥，
              同一时刻只有一个在无障碍树里。此前两者同时被拿掉（按钮 `display:none`、这里
              `aria-hidden`），于是窄窗下读屏用户拿不到任何进度信息。进度条本身仍是装饰。 */}
          <div className="luna-onboarding__stepper-compact">
            <span>{t("progress_compact", { current: index + 1, total: order.length })}</span>
            <span className="luna-onboarding__stepper-bar" aria-hidden="true">
              <span
                className="luna-onboarding__stepper-bar-fill"
                style={cssVariables({
                  "--progress-inline-size": `${Math.round(((index + 1) / order.length) * 100)}%`,
                })}
              />
            </span>
          </div>
        </nav>

        <header className="luna-onboarding__header">
          <h1 ref={headingRef} tabIndex={-1}>
            {t(`step.${step}.title`)}
          </h1>
        </header>

        <p className="luna-onboarding__body">{t(`step.${step}.body`)}</p>

        {/* key = 步骤：切步即整块重挂载，配合 CSS 入场过渡（reduce-motion 下无动画）。 */}
        <div key={step} className="luna-onboarding__content" data-step={step}>
          {renderStepContent()}
        </div>

        {/* 主按钮灰着的原因 + 出路：壳层唯一渲染点（各步只上报，不各画一份）。 */}
        {primary?.blocker ? (
          <p className="luna-onboarding__blocked" role="status">
            <span>{primary.blocker.reason}</span>
            {primary.blocker.fix ? (
              <button
                type="button"
                className="luna-onboarding__inline-action"
                onClick={primary.blocker.fix}
              >
                {primary.blocker.fixLabel}
              </button>
            ) : null}
          </p>
        ) : null}

        <footer className="luna-onboarding__actions">
          <div className="luna-onboarding__actions-left">
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={prev === null}
              onClick={() => {
                if (prev) {
                  goTo(prev);
                }
              }}
            >
              {t("action.back")}
            </button>
            {activeDownload ? (
              <span className="luna-onboarding__download-pill" role="status">
                <span
                  className="luna-onboarding__download-pill-fill"
                  data-indeterminate={activeDownload.percent === null}
                  style={
                    activeDownload.percent === null
                      ? undefined
                      : cssVariables({ "--progress-inline-size": `${activeDownload.percent}%` })
                  }
                />
                <span className="luna-onboarding__download-pill-text">
                  {activeDownload.coreml
                    ? t("download_pill.coreml", { name: activeDownload.name })
                    : activeDownload.percent === null
                      ? t("download_pill.indeterminate", { name: activeDownload.name })
                      : t("download_pill.progress", {
                          name: activeDownload.name,
                          percent: activeDownload.percent,
                          done: formatBytes(activeDownload.done, i18n.language),
                          total: formatBytes(activeDownload.total, i18n.language),
                        })}
                </span>
              </span>
            ) : null}
          </div>

          <div className="luna-onboarding__actions-right">
            {/* Vault 绑定后每一步都能「先进入应用」（PRD-14-041）；最后一步的主按钮就是收场，不重复。 */}
            {finishable && !isLast && !overlay ? (
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => {
                  void complete();
                }}
              >
                {t("action.enter_now")}
              </button>
            ) : null}
            {primary ? (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={primary.blocker !== null}
                onClick={primary.run}
              >
                {primary.label}
              </button>
            ) : null}
          </div>
        </footer>
      </main>
    </div>
  );
}
