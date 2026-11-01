/**
 * 「应用 › 模板」页签（ADR-168 ⑦ / ADR-169 ⑤，离线先行）：笔记模板（内置、装上的模板包带来的、库里
 * 你自己维护的 Markdown，同名时库里的优先）、AI 整理模板，以及装上的模板包，在这里看全、直接用；
 * 在线模板目录可用时还有「获取更多模板」（纵切 7b，`TemplateMarketSection`），不可用时那一块不出现。
 *
 * 全部复用既有命令，不另起一套：笔记模板是 Rust 模板注册表的投影（`note_templates`，与「新建笔记」
 * 的模板选择器同一份）；内置模板按 `luna.type` 建笔记（`create_note`；未安装的应用拥有的类型 Rust
 * 本就不列，ADR-165 ④），其余由 Rust 安全读取与实例化（`create_note_from_template`）；整理模板的
 * 增删改仍在原来的管理框（`DistillTemplateManager`），模板包带来的只读列出。模板正文与路径都不经这里。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  commands,
  type DistillTemplateChoiceDto,
  type NoteTemplateEntryDto,
  type TemplateFileDto,
  type TemplatePackDto,
  type UserTemplateDto,
} from "../../ipc/bindings";
import { useBoolSetting } from "../../hooks/use-bool-setting";
import { openNote } from "../../lib/navigate";
import { DistillTemplateManager } from "../distill/DistillTemplateManager";
import { TemplateMarketSection } from "./TemplateMarketSection";
import { TemplatePacksSection } from "./TemplatePacksSection";
import { useTemplateMarket } from "../../hooks/use-template-market";

/** 页上一句结果提示（文案键）。 */
type TemplateNotice = "apps.templates.createFailed" | "apps.templates.openFolderFailed";

export function TemplatesTab() {
  const { t } = useTranslation("workspace");
  const advancedMode = useBoolSetting("settings.advanced_mode", false);
  const [notes, setNotes] = useState<NoteTemplateEntryDto[]>([]);
  const [distill, setDistill] = useState<UserTemplateDto[]>([]);
  const [packDistill, setPackDistill] = useState<DistillTemplateChoiceDto[]>([]);
  const [packs, setPacks] = useState<TemplatePackDto[]>([]);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<TemplateNotice | null>(null);
  const [managing, setManaging] = useState(false);
  const { market, reload: reloadMarket } = useTemplateMarket();

  const reload = useCallback(() => {
    void Promise.all([
      commands.noteTemplates(),
      commands.distillUserTemplates(),
      commands.distillTemplateChoices(),
      commands.templatePacks(),
    ]).then(([noteList, distillList, choiceList, packList]) => {
      setNotes(noteList);
      setDistill(distillList.status === "ok" ? distillList.data : []);
      setPackDistill(choiceList.status === "ok" ? choiceList.data.filter((choice) => choice.pack_name !== null) : []);
      setPacks(packList);
    });
  }, []);

  useEffect(() => reload(), [reload]);

  const builtin = notes.flatMap((entry) => (entry.note_type !== null ? [entry.note_type] : []));
  const fromPacks = notes.filter((entry) => entry.layer === "content");
  const mine = notes.filter((entry) => entry.layer === "vault");

  /** 用一个模板新建笔记，建好就打开它。 */
  const create = async (make: () => Promise<{ status: "ok"; data: { path: string } } | { status: "error"; error: string }>) => {
    setBusy(true);
    setNotice(null);
    const result = await make();
    setBusy(false);
    if (result.status === "ok") openNote(result.data.path);
    else setNotice("apps.templates.createFailed");
  };

  const openFolder = async () => {
    setNotice(null);
    const result = await commands.openUserTemplatesDir();
    if (result.status === "error") setNotice("apps.templates.openFolderFailed");
  };

  /** 一行「包里的 / 我的」笔记模板。 */
  const fileRow = (entry: NoteTemplateEntryDto, meta: string) => {
    const file: TemplateFileDto | null = entry.file;
    const name = entry.name ?? entry.key;
    if (file === null) return null;
    return (
      <li key={entry.key} className="luna-apps__row">
        <span className="luna-apps__card-text">
          <span className="luna-apps__card-name">{name}</span>
          <span className="luna-apps__meta">{meta}</span>
        </span>
        <span className="luna-apps__row-side">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            aria-label={t("apps.rowAction", { action: t("apps.templates.use"), name })}
            onClick={() => void create(() => commands.createNoteFromTemplate(file))}
          >
            {t("apps.templates.use")}
          </button>
        </span>
      </li>
    );
  };

  return (
    <div className="luna-apps__panel">
      <p className="luna-apps__lede">{t("apps.templates.lede")}</p>
      {notice !== null ? (
        <p className="luna-apps__error" role="alert">
          {t(notice)}
        </p>
      ) : null}

      <section className="luna-apps__section" aria-labelledby="luna-apps-templates-builtin">
        <h2 id="luna-apps-templates-builtin">{t("apps.templates.builtin.title")}</h2>
        <ul className="luna-apps__list">
          {builtin.map((noteType) => {
            const name = t(`template.${noteType}` as const);
            return (
              <li key={noteType} className="luna-apps__row">
                <span className="luna-apps__card-text">
                  <span className="luna-apps__card-name">{name}</span>
                  <span className="luna-apps__card-summary">{t(`template.${noteType}_desc` as const)}</span>
                </span>
                <span className="luna-apps__row-side">
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    disabled={busy}
                    aria-label={t("apps.rowAction", { action: t("apps.templates.use"), name })}
                    onClick={() => void create(() => commands.createNote(noteType))}
                  >
                    {t("apps.templates.use")}
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      </section>

      {fromPacks.length > 0 ? (
        <section className="luna-apps__section" aria-labelledby="luna-apps-templates-from-packs">
          <h2 id="luna-apps-templates-from-packs">{t("apps.templates.fromPacks.title")}</h2>
          <ul className="luna-apps__list">
            {fromPacks.map((entry) => fileRow(entry, t("template.fromPack", { name: entry.pack_name ?? "" })))}
          </ul>
        </section>
      ) : null}

      <section className="luna-apps__section" aria-labelledby="luna-apps-templates-mine">
        <div className="luna-apps__updates">
          <h2 id="luna-apps-templates-mine">{t("apps.templates.mine.title")}</h2>
          <button type="button" className="luna-btn luna-btn--secondary" onClick={() => void openFolder()}>
            {t("apps.templates.mine.openFolder")}
          </button>
        </div>
        {mine.length > 0 ? (
          <ul className="luna-apps__list">
            {mine.map((entry) =>
              fileRow(
                entry,
                entry.shadows.length > 0
                  ? t("template.shadowsOthers")
                  : entry.file?.layer === "vault"
                    ? entry.file.path
                    : "",
              ),
            )}
          </ul>
        ) : (
          <p className="luna-apps__note">{t("apps.templates.mine.empty")}</p>
        )}
      </section>

      <section className="luna-apps__section" aria-labelledby="luna-apps-templates-distill">
        <div className="luna-apps__updates">
          <h2 id="luna-apps-templates-distill">{t("apps.templates.distill.title")}</h2>
          <button type="button" className="luna-btn luna-btn--secondary" onClick={() => setManaging(true)}>
            {t("apps.templates.distill.manage")}
          </button>
        </div>
        {distill.length + packDistill.length > 0 ? (
          <ul className="luna-apps__list">
            {packDistill.map((choice) => (
              <li key={choice.id} className="luna-apps__row">
                <span className="luna-apps__card-text">
                  <span className="luna-apps__card-name">{choice.name}</span>
                  <span className="luna-apps__meta">{t("template.fromPack", { name: choice.pack_name ?? "" })}</span>
                </span>
              </li>
            ))}
            {distill.map((template) => (
              <li key={template.id} className="luna-apps__row">
                <span className="luna-apps__card-text">
                  <span className="luna-apps__card-name">{template.name}</span>
                  <span className="luna-apps__meta">
                    {t("apps.templates.distill.sections", { count: template.sections.length })}
                  </span>
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="luna-apps__note">{t("apps.templates.distill.empty")}</p>
        )}
      </section>

      <TemplateMarketSection
        market={market}
        onChanged={() => {
          reload();
          reloadMarket();
        }}
      />

      <TemplatePacksSection
        packs={packs}
        advancedMode={advancedMode}
        onChanged={() => {
          reload();
          reloadMarket();
        }}
      />

      {managing ? <DistillTemplateManager onClose={() => setManaging(false)} onChanged={setDistill} /> : null}
    </div>
  );
}
