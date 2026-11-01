/**
 * 批量整理知识笔记的确认卡（PRD-06-044 批量入口 / ADR-024）。
 *
 * 两步流程而非「点了就跑」：预检先把代价摆出来——要整理多少篇、多少篇已最新会跳过、
 * **多少篇因为你编辑过而不会被碰**。最后这个数字是本卡与重扫确认卡最大的不同：它不是
 * "顺便说一句"，而是这条能力最重要的承诺（绝不覆盖用户资产）的可见证据。
 *
 * 与重扫卡刻意的差别：**只有一个开始按钮**。重扫有「只补缺口 / 全部重新生成」两档，
 * 是因为换模型后重刷标签是真实场景；而蒸馏"全部重新生成"意味着把已经好好的纪要
 * 重跑一遍 LLM，代价高、收益近乎零，且一旦用户编辑过就必须跳过——那一档只会诱导误操作。
 */

import { useEffect, useState } from "react";
import { SelectBox } from "../../components/ui/SelectBox";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type DistillTemplateChoiceDto, type UsageEstimateDto } from "../../ipc/bindings";
import { useFailureGuidance } from "../../hooks/use-failure-guidance";
import { goTo } from "../../lib/navigate";
import { estimateLine, shortfallFailureCode } from "../../lib/task-estimate";
import type { DistillBatchController } from "./use-distill-batch";
import { GetMoreTemplates } from "../../components/GetMoreTemplates";

/** 「自动」哨兵：不下发模板，逐篇由 Rust 按「已记录 > 建议」解析。 */
const AUTO = "__auto__";

export function DistillBatchDialog({ controller }: { controller: DistillBatchController }) {
  const { t, i18n } = useTranslation("workspace");
  const { dialog, closeDialog, start } = controller;
  const open = dialog.kind !== "closed";
  // 整批统一的模板选择（PRD-06-051 修订）：默认「自动」维持既有行为；显式选了某个
  // 模板即整批按它来（显式 > 已记录，解析顺序的既有语义）。清单由 Rust 下发（ADR-003），
  // 开卡时取一次。仍不做逐篇覆写与逐篇分布（原裁决不变）。
  const [choices, setChoices] = useState<DistillTemplateChoiceDto[]>([]);
  const [template, setTemplate] = useState<string>(AUTO);
  const [fullText, setFullText] = useState(false);
  // 任务前预估（PRD-13-040 ②）：要不要估、估多少、够不够全由 Rust 答；本机跑 / 未登录 / 问不到时为
  // `null`，卡上什么都不多说、照常可以开始（预估不是闸，服务端预留才是）。
  const [estimate, setEstimate] = useState<UsageEstimateDto | null>(null);
  const readyScope = dialog.kind === "ready" ? dialog.scope : undefined;
  const estimateFullText = template === AUTO ? null : fullText;
  useEffect(() => {
    setEstimate(null);
    if (readyScope === undefined) {
      return;
    }
    let live = true;
    void commands
      .taskEstimate({ kind: "distill_scope", scope: readyScope, full_text: estimateFullText })
      .then((value) => {
        if (live) setEstimate(value);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [readyScope, estimateFullText]);
  const shortfall = estimate?.outcome.state === "insufficient" ? estimate.outcome.reason : null;
  // 不够时的修复按钮与失败卡同一取法：额度不够 → 升级或购买（卖的话）/ 打开账户；撞上自设上限 → 调整上限。
  const fix = useFailureGuidance(shortfall === null ? null : shortfallFailureCode(shortfall));

  useEffect(() => {
    if (!open) {
      return;
    }
    // 每次开卡回到「自动」：模板选择是**这一批**的决定，不是一项记住的偏好——
    // 上次选过讲述式不代表这次这个文件夹也该是讲述式。
    setTemplate(AUTO);
    setFullText(false);
    void commands.distillTemplateChoices().then((result) => {
      if (result.status === "ok") {
        setChoices(result.data);
      }
    });
  }, [open]);

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，且它还搭在「开卡时重置模板选项」那个效果里——两件事共用一次订阅，
  // 于是改其中一件的依赖就会顺手影响另一件。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: closeDialog, enabled: open });

  if (dialog.kind === "closed") {
    return null;
  }

  const renderBody = () => {
    if (dialog.kind === "loading") {
      return <p className="luna-dialog__note">{t("distill.batchPreflight")}</p>;
    }
    if (dialog.kind === "error") {
      return (
        <p className="luna-field-error" role="alert">
          {dialog.message}
        </p>
      );
    }
    const { preflight, startError, scope } = dialog;
    const estimateText = estimate === null ? null : estimateLine(estimate, t, i18n.language);
    return (
      <>
        {/* 队列里已有蒸馏任务：如实告知，但**不再拦着不让开始**（PRD-06-051 修订）——
            队列本就是顺序执行的，再排几件正是它该做的事；初版那句"已有一次在进行中"
            源自批量自建 runner 的单例限制，那套机构已经退场。 */}
        {preflight.running && <p className="luna-dialog__note">{t("distill.batchQueueBusy")}</p>}
        <ul className="luna-dialog__counts">
          <li>
            <strong>{preflight.pending}</strong>
            <span>{t("distill.batchPending")}</span>
          </li>
          {preflight.fresh > 0 && (
            <li>
              <strong>{preflight.fresh}</strong>
              <span>{t("distill.batchFresh")}</span>
            </li>
          )}
          {/* 「你编辑过所以我不碰」——这条能力最重要的承诺，必须看得见。 */}
          {preflight.edited > 0 && (
            <li>
              <strong>{preflight.edited}</strong>
              <span>{t("distill.batchEdited")}</span>
            </li>
          )}
          {preflight.empty > 0 && (
            <li>
              <strong>{preflight.empty}</strong>
              <span>{t("distill.batchEmpty")}</span>
            </li>
          )}
        </ul>
        {/* 整批模板：默认「自动（按各篇内容适配）」。选中具体模板时才显示整理稿勾选
            ——「自动」下各篇有各自的默认值，一个全局勾选框说不清它管谁。 */}
        <label className="luna-distill-options__choice luna-distill-batch__template">
          <span className="luna-distill-options__choice-name">{t("distill.batchTemplate")}</span>
          <SelectBox
            className="luna-distill-batch__template-select"
            ariaLabel={t("distill.batchTemplate")}
            value={template}
            onChange={(event) => {
              const next = event.target.value;
              setTemplate(next);
              setFullText(choices.find((choice) => choice.id === next)?.full_text_default ?? false);
            }}
          >
            <option value={AUTO}>{t("distill.batchTemplateAuto")}</option>
            {choices.map((choice) => (
              <option key={choice.id} value={choice.id}>
                {choice.name ?? t(`distill.template.${choice.id}`)}
              </option>
            ))}
          </SelectBox>
        </label>
        <GetMoreTemplates onNavigate={closeDialog} />
        {template !== AUTO && (
          <label className="luna-distill-options__toggle">
            <input
              type="checkbox"
              checked={fullText}
              onChange={(event) => setFullText(event.target.checked)}
            />
            <span className="luna-distill-options__choice-body">
              <span className="luna-distill-options__choice-name">{t("distill.fullText")}</span>
              <span className="luna-distill-options__choice-desc">{t("distill.fullTextCost")}</span>
            </span>
          </label>
        )}
        <p className="luna-dialog__note">{t("distill.batchCost")}</p>
        {/* 约消耗多少、从哪本账扣——「约」字在文案里，最终以服务端计量为准。 */}
        {estimateText !== null && <p className="luna-dialog__note">{estimateText}</p>}
        {estimateText !== null && <p className="luna-dialog__note">{t("cloud:estimate.note")}</p>}
        {/* 不够：开始之前就说清，而不是跑到一半失败（开始按钮随之不可点）。 */}
        {shortfall !== null && (
          <p className="luna-dialog__note" role="status">
            {t(`cloud:estimate.short.${shortfall}.body`)}
          </p>
        )}
        {preflight.pending === 0 && (
          <p className="luna-dialog__note">{t("distill.batchNothing")}</p>
        )}
        {startError && (
          <p className="luna-field-error" role="alert">
            {startError}
          </p>
        )}
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={closeDialog}>
            {t("distill.batchCancel")}
          </button>
          {shortfall !== null && fix !== null && (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => {
                closeDialog();
                goTo(fix.route);
              }}
            >
              {t(fix.labelKey)}
            </button>
          )}
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={preflight.pending === 0 || shortfall !== null}
            onClick={() =>
              void start(
                scope,
                template === AUTO ? null : template,
                template === AUTO ? null : fullText,
              )
            }
          >
            {t("distill.batchStart", { count: preflight.pending })}
          </button>
        </div>
      </>
    );
  };

  return (
    <div className="luna-overlay" role="presentation" onClick={closeDialog}>
      <div
        ref={dialogRef}
        className="luna-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("distill.batchTitle")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3>{t("distill.batchTitle")}</h3>
          {/* 范围陈述：定向整理时明示对象，杜绝"以为在整理整库"。 */}
          <p className="luna-dialog__scope">
            {dialog.scope === null
              ? t("distill.batchScopeAll")
              : t("distill.batchScopeFolder", { folder: dialog.scope })}
          </p>
          <p>{t("distill.batchDesc")}</p>
        </header>
        {renderBody()}
      </div>
    </div>
  );
}
