/**
 * 就地加词弹窗（PRD-07-061 / PRD-02 F-GAP-01）。
 *
 * **为什么需要它**：个人词典此前只有一条入口——设置里的那张表。而用户是**在看见错字的
 * 那一刻**才知道自己需要一条词条：转写把人名认错、语音输入打出个同音词。让他记住这件事、
 * 翻到设置、再手打一遍原文，等于把这条需求丢在原地（真机反馈「路径太深，想要找到非常困难」）。
 *
 * 故本组件是**唯一宿主**：任何内容面（笔记正文、回放字幕）选中文本后右键「添加到个人词典」，
 * 都只派发一个 `luna:glossary-add` 事件，由这里接住、预填、落库——各表面不必各写一份弹窗，
 * 也不必各自知道词典存在哪儿。
 *
 * 落库经 Rust `upsert_glossary_entry`（去重与覆盖规则在 Rust，ADR-003）；覆盖时把旧值
 * 回显给用户，而不是静默改掉一条他可能忘了自己加过的词。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands } from "../../ipc/bindings";
import { GLOSSARY_ADD_EVENT, type GlossaryAddDetail } from "../../lib/app-events";
import { isImeKeyEvent } from "../../lib/ime-key-event";

/** 保存结果回执：`null` = 尚未保存。 */
type Receipt =
  { kind: "added" } | { kind: "replaced"; previous: string } | { kind: "error"; message: string };

export function GlossaryQuickAdd() {
  const { t } = useTranslation("settings");
  const [open, setOpen] = useState(false);
  const [term, setTerm] = useState("");
  const [canonical, setCanonical] = useState("");
  const [saving, setSaving] = useState(false);
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const canonicalRef = useRef<HTMLInputElement | null>(null);

  const close = useCallback(() => {
    setOpen(false);
    setReceipt(null);
  }, []);

  useEffect(() => {
    const onAdd = (event: Event) => {
      const detail = (event as CustomEvent<GlossaryAddDetail>).detail;
      setTerm(detail.term);
      // 正确形式留空——预填成和原始词一样只会诱使用户直接保存一条无效条目。
      setCanonical("");
      setReceipt(null);
      setOpen(true);
    };
    window.addEventListener(GLOSSARY_ADD_EVENT, onAdd);
    return () => window.removeEventListener(GLOSSARY_ADD_EVENT, onAdd);
  }, []);

  // 焦点落在「正确形式」而不是第一个输入框：原始词已由选区填好，用户要打的是另一栏。
  useEffect(() => {
    if (open) {
      canonicalRef.current?.focus();
    }
  }, [open]);

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，没有焦点圈定也没有焦点归还（十二张卡各写各的，能力参差）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: close, enabled: open });

  if (!open) {
    return null;
  }

  const canSave = term.trim().length > 0 && canonical.trim().length > 0 && !saving;

  async function save() {
    if (!canSave) {
      return;
    }
    setSaving(true);
    const result = await commands.upsertGlossaryEntry(term, canonical);
    setSaving(false);
    if (result.status === "error") {
      setReceipt({ kind: "error", message: result.error });
      return;
    }
    const previous = result.data.previous;
    setReceipt(previous === null ? { kind: "added" } : { kind: "replaced", previous });
    // 成功即关闭：这是一个"顺手做掉"的动作，停在弹窗上会打断用户原本在做的事。
    // 覆盖了旧值时多留一拍，好让那句回执被看见。
    window.setTimeout(close, previous === null ? 350 : 1600);
  }

  return (
    <div className="luna-overlay" role="presentation" onClick={() => !saving && close()}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-glossary-add"
        role="dialog"
        aria-modal="true"
        aria-label={t("glossary.quickAdd.title")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-glossary-add__header">
          <strong>{t("glossary.quickAdd.title")}</strong>
          <p>{t("glossary.quickAdd.desc")}</p>
        </header>

        <label className="luna-glossary-add__field">
          <span>{t("glossary.col.term")}</span>
          <input
            type="text"
            className="luna-input"
            value={term}
            placeholder={t("glossary.placeholder.term")}
            onChange={(event) => setTerm(event.currentTarget.value)}
          />
        </label>
        <label className="luna-glossary-add__field">
          <span>{t("glossary.col.canonical")}</span>
          <input
            ref={canonicalRef}
            type="text"
            className="luna-input"
            value={canonical}
            placeholder={t("glossary.placeholder.canonical")}
            onChange={(event) => setCanonical(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (isImeKeyEvent(event)) return;
              if (event.key === "Enter") {
                void save();
              }
            }}
          />
        </label>

        {receipt?.kind === "replaced" && (
          <p className="luna-glossary-add__receipt" role="status">
            {t("glossary.quickAdd.replaced", { previous: receipt.previous })}
          </p>
        )}
        {receipt?.kind === "error" && (
          <p className="luna-glossary-add__error" role="alert">
            {receipt.message}
          </p>
        )}

        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={close}>
            {t("glossary.quickAdd.cancel")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={!canSave}
            onClick={() => void save()}
          >
            {saving ? t("glossary.quickAdd.saving") : t("glossary.quickAdd.save")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 派发就地加词请求（各内容面共用；不必知道弹窗在哪儿）。 */
export function requestGlossaryAdd(term: string): void {
  window.dispatchEvent(
    new CustomEvent<GlossaryAddDetail>(GLOSSARY_ADD_EVENT, { detail: { term } }),
  );
}
