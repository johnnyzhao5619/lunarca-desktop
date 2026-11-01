// 模板选择器（PRD-06-015 P1）：Rust 笔记模板注册表的投影（ADR-169 ⑤）——内置归档骨架、装上的
// 模板包带来的、用户维护的普通 Markdown 模板，同名时库里的优先（被盖住的不列）。
// 模板只在显式选择时由 Rust 安全读取/实例化；本组件不触碰 Vault 路径或模板正文。

import { useEffect, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { CloseIcon } from "../../components/ui/Icon";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type NoteTemplateEntryDto, type NoteTypeDto, type TemplateFileDto } from "../../ipc/bindings";
import { GetMoreTemplates } from "../../components/GetMoreTemplates";

interface TemplatePickerProps {
  open: boolean;
  onClose: () => void;
  onCreated: (path: string) => void;
  onError: (error: string) => void;
}

export function TemplatePicker({ open, onClose, onCreated, onError }: TemplatePickerProps) {
  const { t } = useTranslation("workspace");
  const [entries, setEntries] = useState<NoteTemplateEntryDto[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    void commands.noteTemplates().then((list) => {
      setEntries(list);
      setLoading(false);
    });
  }, [open]);

  const builtin = entries.flatMap((entry) => (entry.note_type !== null ? [entry.note_type] : []));
  const fromPacks = entries.filter((entry) => entry.layer === "content");
  const mine = entries.filter((entry) => entry.layer === "vault");

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，没有焦点圈定也没有焦点归还（十二张卡各写各的，能力参差）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, enabled: open });
  async function handleBuiltin(noteType: NoteTypeDto) {
    setLoading(true);
    const result = await commands.createNote(noteType);
    if (result.status === "ok") {
      onCreated(result.data.path);
      onClose();
    } else {
      onError(t("template.error", { error: result.error }));
    }
    setLoading(false);
  }

  async function handleFile(file: TemplateFileDto) {
    setLoading(true);
    const result = await commands.createNoteFromTemplate(file);
    if (result.status === "ok") {
      onCreated(result.data.path);
      onClose();
    } else {
      onError(t("template.error", { error: result.error }));
    }
    setLoading(false);
  }

  /** 一张「包里的 / 我的」模板卡。 */
  const fileCard = (entry: NoteTemplateEntryDto, detail: string, modifier: string) => {
    const file = entry.file;
    if (file === null) return null;
    return (
      <button
        key={entry.key}
        type="button"
        className={`luna-template-card ${modifier}`}
        disabled={loading}
        onClick={() => void handleFile(file)}
      >
        <strong className="luna-template-card__name">{entry.name}</strong>
        <span className="luna-template-card__desc">{detail}</span>
      </button>
    );
  };

  async function openTemplateDirectory() {
    const result = await commands.openUserTemplatesDir();
    if (result.status === "error") {
      onError(t("template.manageError", { error: result.error }));
    }
  }

  if (!open) return null;

  return (
    <div className="luna-overlay" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-template-picker"
        role="dialog"
        aria-modal="true"
        aria-labelledby="luna-template-picker-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-template-picker__header">
          <div>
            <p className="luna-template-picker__eyebrow">{t("template.eyebrow")}</p>
            <h2 id="luna-template-picker-title">{t("template.title")}</h2>
          </div>
          <IconButton
            label={t("template.cancel")}
            icon={<CloseIcon size={14} />}
            onClick={onClose}
          />
        </header>

        {loading && entries.length === 0 ? (
          <p className="luna-empty">{t("common:status.loading")}</p>
        ) : (
          <div className="luna-template-picker__content">
            <section className="luna-template-picker__group">
              <div className="luna-template-picker__group-heading">
                <div>
                  <h3>{t("template.builtinTitle")}</h3>
                  <p>{t("template.builtinHint")}</p>
                </div>
              </div>
              <div className="luna-template-picker__grid">
                {builtin.map((noteType) => (
                  <button
                    key={noteType}
                    type="button"
                    className="luna-template-card"
                    disabled={loading}
                    onClick={() => void handleBuiltin(noteType)}
                  >
                    <strong className="luna-template-card__name">
                      {t(`template.${noteType}` as const)}
                    </strong>
                    <span className="luna-template-card__desc">
                      {t(`template.${noteType}_desc` as const)}
                    </span>
                  </button>
                ))}
              </div>
            </section>

            {fromPacks.length > 0 ? (
              <section className="luna-template-picker__group">
                <div className="luna-template-picker__group-heading">
                  <div>
                    <h3>{t("template.packTitle")}</h3>
                    <p>{t("template.packHint")}</p>
                  </div>
                </div>
                <div className="luna-template-picker__grid">
                  {fromPacks.map((entry) =>
                    fileCard(entry, t("template.fromPack", { name: entry.pack_name ?? "" }), "luna-template-card--user"),
                  )}
                </div>
              </section>
            ) : null}

            <section className="luna-template-picker__group luna-template-picker__group--user">
              <div className="luna-template-picker__group-heading">
                <div>
                  <h3>{t("template.userTitle")}</h3>
                  <p>{t("template.userHint")}</p>
                </div>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-template-picker__manage"
                  disabled={loading}
                  onClick={() => void openTemplateDirectory()}
                >
                  {t("template.manage")}
                </button>
              </div>
              {mine.length === 0 ? (
                <div className="luna-template-picker__empty">
                  <p>{t("template.userEmpty")}</p>
                  <code>{t("template.variables")}</code>
                </div>
              ) : (
                <div className="luna-template-picker__grid">
                  {mine.map((entry) =>
                    fileCard(
                      entry,
                      entry.shadows.length > 0
                        ? t("template.shadowsOthers")
                        : entry.file?.layer === "vault"
                          ? entry.file.path
                          : "",
                      "luna-template-card--user",
                    ),
                  )}
                </div>
              )}
            </section>
          </div>
        )}

        <footer className="luna-template-picker__footer">
          <GetMoreTemplates onNavigate={onClose} />
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
            {t("template.cancel")}
          </button>
        </footer>
      </div>
    </div>
  );
}
