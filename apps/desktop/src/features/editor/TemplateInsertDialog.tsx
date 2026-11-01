/**
 * 「插入模板」选择框（PRD-06-015，笔记库对标轮 V6）。
 *
 * 斜杠菜单、右键「插入 ▸」与命令面板三个入口都打开它（同一个动作源 `{ kind: "template" }`）。
 * 列笔记模板注册表里的 Markdown 模板（库里的与装上的模板包带来的，ADR-169 ⑤）——内置模板是新建
 * 归档笔记的骨架，插进正文没有意义。模板的读取与渲染全在 Rust，这里只让用户选，选中的那份交回
 * 调用方；外观沿用「从模板新建」那张卡（同一类东西）。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon } from "../../components/ui/Icon";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type NoteTemplateEntryDto, type TemplateFileDto } from "../../ipc/bindings";
import { GetMoreTemplates } from "../../components/GetMoreTemplates";

export function TemplateInsertDialog({
  onPick,
  onClose,
  onError,
}: {
  /** 选中一份模板。 */
  onPick: (template: TemplateFileDto) => void;
  onClose: () => void;
  onError: (message: string) => void;
}) {
  const { t } = useTranslation("editor");
  const [templates, setTemplates] = useState<NoteTemplateEntryDto[] | null>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, enabled: true });

  useEffect(() => {
    void commands.noteTemplates().then((list) => {
      setTemplates(list.filter((entry) => entry.file !== null));
    });
  }, []);

  async function openTemplateDirectory() {
    const result = await commands.openUserTemplatesDir();
    if (result.status === "error") {
      onError(t("workspace:template.manageError", { error: result.error }));
    }
  }

  return (
    <div className="luna-overlay" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-template-picker"
        role="dialog"
        aria-modal="true"
        aria-labelledby="luna-template-insert-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-template-picker__header">
          <div>
            <h2 id="luna-template-insert-title">{t("templateInsert.title")}</h2>
          </div>
          <IconButton
            label={t("workspace:template.cancel")}
            icon={<CloseIcon size={14} />}
            onClick={onClose}
          />
        </header>
        {templates === null ? (
          <p className="luna-empty">{t("common:status.loading")}</p>
        ) : (
          <div className="luna-template-picker__content">
            <section className="luna-template-picker__group luna-template-picker__group--user">
              <div className="luna-template-picker__group-heading">
                <div>
                  <p>{t("templateInsert.hint")}</p>
                </div>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-template-picker__manage"
                  onClick={() => void openTemplateDirectory()}
                >
                  {t("workspace:template.manage")}
                </button>
              </div>
              {templates.length === 0 ? (
                <div className="luna-template-picker__empty">
                  <p>{t("workspace:template.userEmpty")}</p>
                  <code>{t("workspace:template.variables")}</code>
                </div>
              ) : (
                <div className="luna-template-picker__grid">
                  {templates.map((template) => {
                    const file = template.file;
                    if (file === null) return null;
                    return (
                      <button
                        key={template.key}
                        type="button"
                        className="luna-template-card luna-template-card--user"
                        onClick={() => onPick(file)}
                      >
                        <strong className="luna-template-card__name">{template.name}</strong>
                        <span className="luna-template-card__desc">
                          {file.layer === "content"
                            ? t("workspace:template.fromPack", { name: template.pack_name ?? "" })
                            : file.path}
                        </span>
                      </button>
                    );
                  })}
                </div>
              )}
            </section>
          </div>
        )}
        <footer className="luna-template-picker__footer">
          <GetMoreTemplates onNavigate={onClose} />
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
            {t("workspace:template.cancel")}
          </button>
        </footer>
      </div>
    </div>
  );
}
