/**
 * 「应用 › 已安装」里的模板包（纵切 7b B2，PRD-18-011 / 016）：装上的模板包——从文件导入的与从在线目录获取的同一种行——
 * 列在已装应用与主题下面。「可更新到哪一版」是 `template_packs` 的 `update_to`，由 Rust 用与「获取更多模板」同一份市场
 * 状态算出（零网络，目录不可用时一条也不标），前端不比版本；可更新的给「更新」（与「获取更多模板」同一个确认框），
 * 都能卸载。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { useConfirm } from "../../components/ConfirmHost";
import { commands, type TemplateMarketDto, type TemplatePackDto } from "../../ipc/bindings";
import { MarketNoticeLine, useMarketTemplateGet } from "./TemplateMarketSection";

export function InstalledTemplatePacks({
  packs,
  market,
  onChanged,
}: {
  packs: readonly TemplatePackDto[];
  market: TemplateMarketDto | null | undefined;
  /** 更新或卸载之后：重取已装清单与市场状态。 */
  onChanged: () => void;
}) {
  const { t } = useTranslation("workspace");
  const confirm = useConfirm();
  const { get, busy, notice } = useMarketTemplateGet(onChanged);
  const [failed, setFailed] = useState(false);
  if (packs.length === 0) return null;

  const uninstall = async (pack: TemplatePackDto) => {
    const ok = await confirm({
      title: t("apps.templates.packs.uninstall.title", { name: pack.name }),
      description: t("apps.templates.packs.uninstall.description"),
      confirmLabel: t("apps.templates.packs.uninstall.confirm"),
      destructive: true,
    });
    if (!ok) return;
    setFailed(false);
    const result = await commands.uninstallTemplatePack(pack.id);
    if (result.status === "ok") onChanged();
    else setFailed(true);
  };

  return (
    <section className="luna-apps__section" aria-labelledby="luna-apps-installed-template-packs">
      <h2 id="luna-apps-installed-template-packs">{t("apps.templates.packs.title")}</h2>
      <MarketNoticeLine notice={notice} />
      {failed ? (
        <p className="luna-apps__error" role="alert">
          {t("apps.templates.packs.failed")}
        </p>
      ) : null}
      <ul className="luna-apps__list">
        {packs.map((pack) => {
          // 更新走「获取更多模板」那一条（确认框要它的能力与多出的能力）；目录里没有这一条时不给按钮。
          const listed = pack.update_to !== null ? market?.packs.find((item) => item.id === pack.id) : undefined;
          return (
            <li key={pack.id} className="luna-apps__row">
              <span className="luna-apps__card-text">
                <span className="luna-apps__card-name">{pack.name}</span>
                <span className="luna-apps__meta">
                  {pack.update_to !== null
                    ? t("apps.templates.market.updateMeta", {
                        publisher: pack.publisher,
                        license: pack.license,
                        from: pack.version,
                        to: pack.update_to,
                      })
                    : t("apps.templates.packs.meta", { publisher: pack.publisher, license: pack.license, version: pack.version })}
                </span>
              </span>
              <span className="luna-apps__row-side">
                {listed !== undefined ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--primary"
                    aria-label={t("apps.rowAction", { action: t("apps.templates.market.update"), name: pack.name })}
                    disabled={busy !== null}
                    onClick={() => void get(listed)}
                  >
                    {busy === pack.id ? t("apps.templates.market.getting") : t("apps.templates.market.update")}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  aria-label={t("apps.rowAction", { action: t("apps.templates.packs.uninstall.action"), name: pack.name })}
                  disabled={busy === pack.id}
                  onClick={() => void uninstall(pack)}
                >
                  {t("apps.templates.packs.uninstall.action")}
                </button>
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
