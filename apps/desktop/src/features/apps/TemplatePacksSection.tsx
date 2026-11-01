/**
 * 「应用 › 模板」里的「模板包」一节（ADR-169 ⑤ ⑦，PRD-18-024）：装上的模板包、它们带来的东西与它们
 * 会做的事，以及**只在高级模式下出现**的「从文件导入…」。
 *
 * 判定都在 Rust：选文件并预览（`pick_template_pack`：解析、逐项校验、推导能力，不写任何东西），
 * 确认后安装（`install_template_pack`：再读一次同一个文件，sha256 对得上才装）。确认框写明「未经
 * 审核」与推导出的能力；包里的工作流配方只列出来，不执行（执行随在线模板包）。
 */

import { createElement, useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import { commands, type PackCapabilitiesDto, type TemplatePackDto } from "../../ipc/bindings";
import { useConfirm } from "../../components/ConfirmHost";

/** 页上一句结果提示（文案键）。 */
type PackNotice =
  | { key: "apps.templates.packs.installed"; name: string }
  | {
      key:
        | "apps.templates.packs.rejected"
        | "apps.templates.packs.incompatible"
        | "apps.templates.packs.alreadyInstalled"
        | "apps.templates.packs.changed"
        | "apps.templates.packs.failed";
    };

/** 导入失败的稳定错误码 → 页上那句话。 */
function importNotice(error: string): PackNotice {
  if (error.startsWith("TEMPLATE_PACK_REJECTED")) return { key: "apps.templates.packs.rejected" };
  if (error.startsWith("TEMPLATE_PACK_INCOMPATIBLE")) return { key: "apps.templates.packs.incompatible" };
  if (error.startsWith("TEMPLATE_PACK_ALREADY_INSTALLED")) return { key: "apps.templates.packs.alreadyInstalled" };
  if (error.startsWith("TEMPLATE_PACK_CHANGED")) return { key: "apps.templates.packs.changed" };
  return { key: "apps.templates.packs.failed" };
}

/** 它会做的事（由 Rust 从包内容推导）：一条一句。 */
export function capabilityLines(capabilities: PackCapabilitiesDto, t: TFunction<"workspace">): string[] {
  const lines: string[] = [];
  if (capabilities.uses_ai) lines.push(t("apps.templates.packs.can.usesAi"));
  if (capabilities.transcribes) lines.push(t("apps.templates.packs.can.transcribes"));
  if (capabilities.translates) lines.push(t("apps.templates.packs.can.translates"));
  for (const dir of capabilities.creates_notes_in) lines.push(t("apps.templates.packs.can.createsNotesIn", { dir }));
  if (capabilities.exports) lines.push(t("apps.templates.packs.can.exports"));
  return lines;
}

/** 包里有什么：一行。 */
function contentsLine(pack: TemplatePackDto, t: TFunction<"workspace">): string {
  return [
    pack.note_templates.length > 0 ? t("apps.templates.packs.has.noteTemplates", { count: pack.note_templates.length }) : null,
    pack.ai_presets.length > 0 ? t("apps.templates.packs.has.aiPresets", { count: pack.ai_presets.length }) : null,
    pack.distill_templates.length > 0
      ? t("apps.templates.packs.has.distillTemplates", { count: pack.distill_templates.length })
      : null,
    pack.recipes.length > 0 ? t("apps.templates.packs.has.recipes", { count: pack.recipes.length }) : null,
  ]
    .filter((part): part is string => part !== null)
    .join(t("apps.templates.packs.has.separator"));
}

/** 导入确认框的正文：未经审核 · 它会 · 包里有 · 配方只展示。 */
function PackConfirmContent({ pack, updatingFrom }: { pack: TemplatePackDto; updatingFrom: string | null }) {
  const { t } = useTranslation("workspace");
  const can = capabilityLines(pack.capabilities, t);
  return (
    <div className="luna-apps__confirm">
      <p className="luna-apps__error" role="note">
        {t("apps.templates.packs.unreviewed")}
      </p>
      <p>{t("apps.templates.packs.meta", { publisher: pack.publisher, license: pack.license, version: pack.version })}</p>
      {updatingFrom !== null ? <p>{t("apps.templates.packs.updating", { from: updatingFrom, to: pack.version })}</p> : null}
      {can.length > 0 ? (
        <>
          <p className="luna-apps__confirm-lead">{t("apps.templates.packs.canLead")}</p>
          <ul className="luna-apps__confirm-list" aria-label={t("apps.templates.packs.canLead")}>
            {can.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </>
      ) : (
        <p>{t("apps.templates.packs.canNothing")}</p>
      )}
      <p>{contentsLine(pack, t)}</p>
      {pack.recipes.length > 0 ? <p className="luna-apps__note">{t("apps.templates.packs.recipesLater")}</p> : null}
    </div>
  );
}

export function TemplatePacksSection({
  packs,
  advancedMode,
  onChanged,
}: {
  packs: readonly TemplatePackDto[];
  advancedMode: boolean;
  /** 装上或卸掉之后：重取这一页的全部清单。 */
  onChanged: () => void;
}) {
  const { t } = useTranslation("workspace");
  const confirm = useConfirm();
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<PackNotice | null>(null);

  // 没开高级模式、也没装过模板包：这一节不出现（从文件导入模板包只在高级模式下开放，PRD-18-024）。
  if (!advancedMode && packs.length === 0) return null;

  const importFromFile = async () => {
    setBusy(true);
    setNotice(null);
    const picked = await commands.pickTemplatePack();
    setBusy(false);
    if (picked.status === "error") {
      setNotice(importNotice(picked.error));
      return;
    }
    // `null` = 用户在选文件时取消了：什么都没发生，不说话。
    const preview = picked.data;
    if (preview === null) return;
    const ok = await confirm({
      title: t("apps.templates.packs.confirmTitle", { name: preview.pack.name }),
      content: createElement(PackConfirmContent, { pack: preview.pack, updatingFrom: preview.installed_version }),
      confirmLabel: t("apps.templates.packs.confirm"),
      destructive: false,
    });
    if (!ok) return;
    setBusy(true);
    const installed = await commands.installTemplatePack(preview.path, preview.sha256);
    setBusy(false);
    if (installed.status === "error") {
      setNotice(importNotice(installed.error));
      return;
    }
    setNotice({ key: "apps.templates.packs.installed", name: installed.data.name });
    onChanged();
  };

  const uninstall = async (pack: TemplatePackDto) => {
    const ok = await confirm({
      title: t("apps.templates.packs.uninstall.title", { name: pack.name }),
      description: t("apps.templates.packs.uninstall.description"),
      confirmLabel: t("apps.templates.packs.uninstall.confirm"),
      destructive: true,
    });
    if (!ok) return;
    setNotice(null);
    const result = await commands.uninstallTemplatePack(pack.id);
    if (result.status === "ok") onChanged();
    else setNotice({ key: "apps.templates.packs.failed" });
  };

  const failed = notice !== null && notice.key !== "apps.templates.packs.installed";
  return (
    <section className="luna-apps__section" aria-labelledby="luna-apps-templates-packs">
      <div className="luna-apps__updates">
        <h2 id="luna-apps-templates-packs">{t("apps.templates.packs.title")}</h2>
        {advancedMode ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={() => void importFromFile()}
          >
            {busy ? t("apps.templates.packs.importing") : t("apps.templates.packs.importFromFile")}
          </button>
        ) : null}
      </div>
      {notice !== null ? (
        <p className={failed ? "luna-apps__error" : "luna-apps__note"} role={failed ? "alert" : "status"}>
          {notice.key === "apps.templates.packs.installed" ? t(notice.key, { name: notice.name }) : t(notice.key)}
        </p>
      ) : null}
      {packs.length > 0 ? (
        <ul className="luna-apps__list" aria-label={t("apps.templates.packs.title")}>
          {packs.map((pack) => {
            const can = capabilityLines(pack.capabilities, t);
            return (
              <li key={pack.id} className="luna-apps__row">
                <span className="luna-apps__card-text">
                  <span className="luna-apps__card-name">{pack.name}</span>
                  <span className="luna-apps__meta">
                    {t("apps.templates.packs.meta", {
                      publisher: pack.publisher,
                      license: pack.license,
                      version: pack.version,
                    })}
                  </span>
                  <span className="luna-apps__card-summary">{contentsLine(pack, t)}</span>
                  {can.length > 0 ? (
                    <span className="luna-apps__meta">{can.join(t("apps.templates.packs.has.separator"))}</span>
                  ) : null}
                  {pack.recipes.length > 0 ? (
                    <span className="luna-apps__meta">
                      {t("apps.templates.packs.recipes", { titles: pack.recipes.join(t("apps.templates.packs.has.separator")) })}
                    </span>
                  ) : null}
                </span>
                <span className="luna-apps__row-side">
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    aria-label={t("apps.rowAction", { action: t("apps.templates.packs.uninstall.action"), name: pack.name })}
                    onClick={() => void uninstall(pack)}
                  >
                    {t("apps.templates.packs.uninstall.action")}
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="luna-apps__note">{t("apps.templates.packs.empty")}</p>
      )}
    </section>
  );
}
