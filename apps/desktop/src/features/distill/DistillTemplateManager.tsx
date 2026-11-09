/**
 * 自定义蒸馏模板管理（PRD-06-051 批次 2）。
 *
 * 入口在**对象所在处**——生成选项浮层里的「管理模板…」，而不是设置页深处：用户想改模板
 * 的那一刻，正是他看着模板选单觉得"这几个都不合适"的时候。
 *
 * 一个模板 = 展示名 + 一列区块（标题 + 给模型的指导语）+ 默认是否附整理稿。**刻意不做
 * 「自由 prompt」式模板**：产出结构是下游多处的契约（来源区由 Rust 拼在末尾、区块决定笔记
 * 长什么样），一段自由文本无法保证模型给出可预期的结构，出了问题也无从判断是模板写坏了
 * 还是模型不听话。
 *
 * 校验在 Rust（`luna_core::validate`，稳定错误码 → 本组件查 `distill.templateError.*`），
 * 前端不重复实现"什么样的模板算合法"（ADR-003）。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type UserTemplateDto } from "../../ipc/bindings";
import { PlusIcon, TrashIcon } from "@lunarca/editor-core/Icon";
import { vaultErrorText } from "../../lib/vault-error";

/** 空模板草稿（新建时的起点：一个区块，让用户立刻看到该填什么）。 */
function blankTemplate(): UserTemplateDto {
  return {
    id: "",
    name: "",
    sections: [{ heading: "", guidance: "" }],
    full_text_default: false,
  };
}

/** 从 Rust 错误串取稳定码（`DISTILL_TEMPLATE_INVALID: empty_name` → `empty_name`）。 */
function templateErrorKey(error: string): string | null {
  const match = /^DISTILL_TEMPLATE_INVALID:\s*(\w+)/.exec(error);
  return match ? match[1] : null;
}

export interface DistillTemplateManagerProps {
  /** 关闭（保存与否都由用户自己决定，不做"未保存"拦截——草稿丢了重写一遍代价很小）。 */
  onClose: () => void;
  /** 清单变化后回流，供浮层就地刷新选单。 */
  onChanged: (templates: UserTemplateDto[]) => void;
}

export function DistillTemplateManager({ onClose, onChanged }: DistillTemplateManagerProps) {
  const { t } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  const [templates, setTemplates] = useState<UserTemplateDto[]>([]);
  const [draft, setDraft] = useState<UserTemplateDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  // 模态行为统一走共享 hook——本卡此前**完全没有 Esc**。Esc 与 ✕/取消同属"明确意图"，
  // 故草稿态照常可关（与遮罩点击的裁决相反，理由见下方 render 处注释）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: onClose });

  // 只在挂载时读一次，且**不**在读取时回流 onChanged：宿主（徽标）随队列事件高频
  // 重渲染，onChanged 是每次都换身份的 inline 回调——load 若依赖它并在读取时回调它，
  // "读取 → 回流 → 宿主重取选项 → 重渲染 → load 身份变 → 再读取"就是一条持续的 IPC
  // 循环。清单只被本对话框的保存/删除改变，回流只属于那两处。
  const load = useCallback(async () => {
    const result = await commands.distillUserTemplates();
    if (result.status === "ok") {
      setTemplates(result.data);
    } else {
      setError(vaultErrorText(result.error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    if (!draft) {
      return;
    }
    setError(null);
    const result = await commands.saveDistillTemplate(draft);
    if (result.status === "ok") {
      setTemplates(result.data);
      onChanged(result.data);
      setDraft(null);
    } else {
      setError(vaultErrorText(result.error));
    }
  };

  const remove = async (id: string) => {
    const result = await commands.deleteDistillTemplate(id);
    if (result.status === "ok") {
      setTemplates(result.data);
      onChanged(result.data);
      if (draft?.id === id) {
        setDraft(null);
      }
    } else {
      setError(vaultErrorText(result.error));
    }
  };

  const errorKey = error ? templateErrorKey(error) : null;

  return (
    // 编辑草稿时点遮罩**不关闭**（同抓取确认卡的裁决：遮罩点击多半是误触，而误触会
    // 丢掉刚打的字；✕/取消是明确意图，始终可关）。列表态无未保存内容，照常可关。
    <div
      className="luna-overlay"
      role="presentation"
      onClick={() => {
        if (draft === null) {
          onClose();
        }
      }}
    >
      <div
        ref={dialogRef}
        className="luna-dialog luna-distill-templates"
        role="dialog"
        aria-modal="true"
        aria-label={t("distill.manageTemplates")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-distill-templates__header">
          <h2>{t("distill.manageTemplates")}</h2>
          <p>{t("distill.manageTemplatesDesc")}</p>
        </header>

        {draft === null ? (
          <>
            <ul className="luna-distill-templates__list">
              {templates.map((template) => (
                <li key={template.id} className="luna-distill-templates__row">
                  <button
                    type="button"
                    className="luna-distill-templates__row-main"
                    onClick={() => setDraft(template)}
                  >
                    <span className="luna-distill-templates__row-name">{template.name}</span>
                    <span className="luna-distill-templates__row-meta">
                      {t("distill.sectionCount", { count: template.sections.length })}
                    </span>
                  </button>
                  <button
                    type="button"
                    className="luna-distill-templates__row-delete"
                    title={tCommon("action.delete")}
                    aria-label={tCommon("action.delete")}
                    onClick={() => void remove(template.id)}
                  >
                    <TrashIcon size={13} />
                  </button>
                </li>
              ))}
            </ul>
            {templates.length === 0 && (
              <p className="luna-distill-templates__empty">{t("distill.noCustomTemplates")}</p>
            )}
            <div className="luna-distill-templates__actions">
              <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
                {tCommon("action.close")}
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                onClick={() => setDraft(blankTemplate())}
              >
                <PlusIcon size={13} />
                <span>{t("distill.newTemplate")}</span>
              </button>
            </div>
          </>
        ) : (
          <>
            <label className="luna-distill-templates__field">
              <span>{t("distill.templateName")}</span>
              <input
                type="text"
                value={draft.name}
                placeholder={t("distill.templateNamePlaceholder")}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </label>

            <fieldset className="luna-distill-templates__sections">
              <legend>{t("distill.templateSections")}</legend>
              {draft.sections.map((section, index) => (
                // 区块顺序即产出顺序，且用户可能写出两个同名标题——下标是这里唯一稳定的
                // 身份（区块本身没有 id，也不需要有）。
                <div key={index} className="luna-distill-templates__section">
                  <input
                    type="text"
                    value={section.heading}
                    placeholder={t("distill.sectionHeadingPlaceholder")}
                    onChange={(event) => {
                      const sections = [...draft.sections];
                      sections[index] = { ...section, heading: event.target.value };
                      setDraft({ ...draft, sections });
                    }}
                  />
                  <input
                    type="text"
                    value={section.guidance}
                    placeholder={t("distill.sectionGuidancePlaceholder")}
                    onChange={(event) => {
                      const sections = [...draft.sections];
                      sections[index] = { ...section, guidance: event.target.value };
                      setDraft({ ...draft, sections });
                    }}
                  />
                  <button
                    type="button"
                    className="luna-distill-templates__row-delete"
                    title={tCommon("action.delete")}
                    aria-label={tCommon("action.delete")}
                    onClick={() =>
                      setDraft({
                        ...draft,
                        sections: draft.sections.filter((_, at) => at !== index),
                      })
                    }
                  >
                    <TrashIcon size={13} />
                  </button>
                </div>
              ))}
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() =>
                  setDraft({
                    ...draft,
                    sections: [...draft.sections, { heading: "", guidance: "" }],
                  })
                }
              >
                <PlusIcon size={13} />
                <span>{t("distill.addSection")}</span>
              </button>
            </fieldset>

            <label className="luna-distill-options__toggle">
              <input
                type="checkbox"
                checked={draft.full_text_default}
                onChange={(event) =>
                  setDraft({ ...draft, full_text_default: event.target.checked })
                }
              />
              <span className="luna-distill-options__choice-body">
                {/* 与生成浮层的「附整理稿」刻意**不同名**：这里设的是**该模板的默认值**，
                    浮层里那条才是本次生成的实际开关——同名会让两处看起来是同一件事设了
                    两遍（真机反馈"设置冗余"即此困惑）。 */}
                <span className="luna-distill-options__choice-name">
                  {t("distill.fullTextDefault")}
                </span>
                <span className="luna-distill-options__choice-desc">
                  {t("distill.fullTextDefaultDesc")}
                </span>
              </span>
            </label>

            {error !== null && (
              <p className="luna-distill-templates__error" role="alert">
                {/* 校验码由 Rust 给（单一判定点），此处只把它翻成人话；未知码如实显示原文
                    而不是兜成一句"保存失败"——那会把可诊断的信息抹掉。 */}
                {errorKey ? t(`distill.templateError.${errorKey}`) : error}
              </p>
            )}

            <div className="luna-distill-templates__actions">
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => {
                  setDraft(null);
                  setError(null);
                }}
              >
                {tCommon("action.cancel")}
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                onClick={() => void save()}
              >
                {tCommon("action.save")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
