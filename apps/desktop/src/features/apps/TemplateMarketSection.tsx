/**
 * 「应用 › 模板」里的「获取更多模板」（纵切 7b B2，PRD-18-004 / 006 / 007 / 011）：在线模板目录里的包、它们带来的东西
 * 与它们会做的事，「获取」与「更新」（「检查更新」只在「应用」页头一处，ADR-170 ③）。
 *
 * 判定都在 Rust：列什么、每一条的状态、更新会多出哪些能力（`template_market`）；获取时 Rust 按 id 重新取那一条、
 * 下载、比 sha256、重算条目与目录比对后才装（`install_market_template`）。确认框只列能力——市场条目经过服务端审核，
 * 不写「未经审核」（那是从文件导入的，ADR-169 ⑦）；新版本能力变多时写清多了什么，用户确认后才更新。
 * 严格本地、未登录或服务端没发布模板目录时这一块不出现（`market === null`，PRD-18-006）。
 */

import { createElement, useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import { useConfirm } from "../../components/ConfirmHost";
import { commands, type MarketTemplateDto, type TemplateMarketDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { capabilityLines } from "./TemplatePacksSection";

/** 页上一句结果提示（文案键）。 */
export type MarketNotice =
  | { key: "apps.templates.market.installed" | "apps.templates.market.updated"; name: string; version: string }
  | {
      key:
        | "apps.templates.market.mismatch"
        | "apps.templates.market.downloadFailed"
        | "apps.templates.market.needsUpdate"
        | "apps.templates.market.strictLocal"
        | "apps.templates.market.failed";
    };

/** 获取失败的稳定错误码 → 页上那句话。 */
export function marketNotice(error: string): MarketNotice {
  if (error.startsWith("TEMPLATE_MARKET_MISMATCH")) return { key: "apps.templates.market.mismatch" };
  if (error.startsWith("TEMPLATE_MARKET_DOWNLOAD_FAILED")) return { key: "apps.templates.market.downloadFailed" };
  if (error.startsWith("TEMPLATE_MARKET_NEEDS_UPDATE")) return { key: "apps.templates.market.needsUpdate" };
  if (error.startsWith("STRICT_LOCAL_BLOCKED")) return { key: "apps.templates.market.strictLocal" };
  return { key: "apps.templates.market.failed" };
}

/** 包里有什么：一行（与「模板包」一节同一套说法）。 */
function contentsLine(pack: MarketTemplateDto, t: TFunction<"workspace">): string {
  const { contents } = pack;
  return [
    contents.note_templates > 0 ? t("apps.templates.packs.has.noteTemplates", { count: contents.note_templates }) : null,
    contents.ai_presets > 0 ? t("apps.templates.packs.has.aiPresets", { count: contents.ai_presets }) : null,
    contents.distill_templates > 0
      ? t("apps.templates.packs.has.distillTemplates", { count: contents.distill_templates })
      : null,
    contents.recipes > 0 ? t("apps.templates.packs.has.recipes", { count: contents.recipes }) : null,
  ]
    .filter((part): part is string => part !== null)
    .join(t("apps.templates.packs.has.separator"));
}

/** 获取 / 更新确认框的正文：谁做的 · 它会（或新版本还会）· 包里有 · 大小。 */
function MarketConfirmContent({ pack }: { pack: MarketTemplateDto }) {
  const { t, i18n } = useTranslation("workspace");
  const can = capabilityLines(pack.capabilities, t);
  const gained = pack.new_capabilities === null ? [] : capabilityLines(pack.new_capabilities, t);
  return (
    <div className="luna-apps__confirm">
      <p>{t("apps.templates.packs.meta", { publisher: pack.publisher, license: pack.license, version: pack.version })}</p>
      {pack.installed_version !== null ? (
        <p>{t("apps.templates.packs.updating", { from: pack.installed_version, to: pack.version })}</p>
      ) : null}
      {gained.length > 0 ? (
        <>
          <p className="luna-apps__confirm-lead">{t("apps.templates.market.gainsLead")}</p>
          <ul className="luna-apps__confirm-list" aria-label={t("apps.templates.market.gainsLead")}>
            {gained.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </>
      ) : null}
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
      <p className="luna-apps__note">
        {t("apps.templates.market.size", { size: formatBytes(pack.bytes, i18n.language) })}
      </p>
    </div>
  );
}

/**
 * 获取 / 更新一个在线模板包：先弹确认框（列能力；更新多出的能力单列），确认后交给 Rust。「获取更多模板」与「已安装」
 * 两处共用这一份，免得两处的确认框各写一套、说法漂开。
 */
export function useMarketTemplateGet(onChanged: () => void) {
  const { t } = useTranslation("workspace");
  const confirm = useConfirm();
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<MarketNotice | null>(null);

  const get = async (pack: MarketTemplateDto) => {
    const updating = pack.state === "update_available";
    const ok = await confirm({
      title: t(updating ? "apps.templates.market.updateTitle" : "apps.templates.market.getTitle", { name: pack.name }),
      content: createElement(MarketConfirmContent, { pack }),
      confirmLabel: t(updating ? "apps.templates.market.update" : "apps.templates.market.get"),
      destructive: false,
    });
    if (!ok) return;
    setBusy(pack.id);
    setNotice(null);
    // 确认框里已经写了新版本多出的能力：这一次就是那次确认。
    const result = await commands.installMarketTemplate(pack.id, pack.new_capabilities !== null);
    setBusy(null);
    if (result.status === "error") {
      setNotice(marketNotice(result.error));
      return;
    }
    setNotice({
      key: updating ? "apps.templates.market.updated" : "apps.templates.market.installed",
      name: result.data.name,
      version: result.data.version,
    });
    onChanged();
  };

  return { get, busy, notice };
}

/** 一句结果提示（成功是状态，失败是警示）。 */
export function MarketNoticeLine({ notice }: { notice: MarketNotice | null }) {
  const { t } = useTranslation("workspace");
  if (notice === null) return null;
  const succeeded = notice.key === "apps.templates.market.installed" || notice.key === "apps.templates.market.updated";
  return (
    <p className={succeeded ? "luna-apps__note" : "luna-apps__error"} role={succeeded ? "status" : "alert"}>
      {notice.key === "apps.templates.market.installed" || notice.key === "apps.templates.market.updated"
        ? t(notice.key, { name: notice.name, version: notice.version })
        : t(notice.key)}
    </p>
  );
}

export function TemplateMarketSection({
  market,
  onChanged,
}: {
  market: TemplateMarketDto | null | undefined;
  /** 装上或更新之后：重取这一页的全部清单（目录变了另由 `templates://changed` 重读）。 */
  onChanged: () => void;
}) {
  const { t, i18n } = useTranslation("workspace");
  const { get, busy, notice } = useMarketTemplateGet(onChanged);

  if (market === null || market === undefined) return null;

  return (
    <section className="luna-apps__section" aria-labelledby="luna-apps-templates-market">
      <h2 id="luna-apps-templates-market">{t("apps.templates.market.title")}</h2>
      <MarketNoticeLine notice={notice} />
      <ul className="luna-apps__list" aria-label={t("apps.templates.market.title")}>
        {market.packs.map((pack) => {
          const can = capabilityLines(pack.capabilities, t);
          return (
            <li key={pack.id} className="luna-apps__row">
              <span className="luna-apps__card-text">
                <span className="luna-apps__card-name">{pack.name}</span>
                <span className="luna-apps__meta">
                  {pack.state === "update_available" && pack.installed_version !== null
                    ? t("apps.templates.market.updateMeta", {
                        publisher: pack.publisher,
                        license: pack.license,
                        from: pack.installed_version,
                        to: pack.version,
                      })
                    : t("apps.templates.packs.meta", { publisher: pack.publisher, license: pack.license, version: pack.version })}
                </span>
                <span className="luna-apps__card-summary">{contentsLine(pack, t)}</span>
                <span className="luna-apps__meta">
                  {can.length > 0 ? can.join(t("apps.templates.packs.has.separator")) : t("apps.templates.market.contentOnly")}
                  {t("apps.templates.packs.has.separator")}
                  {formatBytes(pack.bytes, i18n.language)}
                </span>
              </span>
              <span className="luna-apps__row-side">
                {pack.state === "available" || pack.state === "update_available" ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    disabled={busy !== null}
                    aria-label={t("apps.rowAction", {
                      action: t(pack.state === "available" ? "apps.templates.market.get" : "apps.templates.market.update"),
                      name: pack.name,
                    })}
                    onClick={() => void get(pack)}
                  >
                    {busy === pack.id
                      ? t("apps.templates.market.getting")
                      : t(pack.state === "available" ? "apps.templates.market.get" : "apps.templates.market.update")}
                  </button>
                ) : (
                  <span className="luna-apps__meta">
                    {t(pack.state === "installed" ? "apps.templates.market.isInstalled" : "apps.templates.market.needsNewerApp")}
                  </span>
                )}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
