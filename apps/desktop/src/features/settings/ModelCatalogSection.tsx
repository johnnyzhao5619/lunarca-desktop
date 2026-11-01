/**
 * 模型目录状态（PRD-13-031：版本 / 上次刷新 / 状态）。
 *
 * 它住在模型页而不是账户页：状态说的正是相邻那份清单。目录是**公开签名数据、不属账户会话**，摆在账户页
 * 容易被误读成账户功能。**这里没有刷新按钮**（ADR-170 ③）：打开这一页时 Rust 按 5 分钟重验一次
 * （`revalidate_catalogs`），登录时无条件刷一次；要立刻检查，到「应用」页头点「检查更新」——一次刷的本来
 * 就不只是模型目录。
 *
 * 前端零业务（ADR-003）：验签、过期、防回滚与状态机全在 Rust；本组件只发命令、渲染状态。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FactRow } from "../../components/ui/FactRow";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { InlineDetails } from "../../components/ui/InlineDetails";
import { commands as appCommands } from "../../ipc/bindings";
import { errorMessage } from "../../lib/cloud-error";

import {
  commands,
  type CatalogStatusDto,
  type CloudError,
  type PromptCatalogStatusDto,
} from "../../ipc/cloud-bindings";
import { formatDateTime } from "../../lib/format-datetime";

/** CloudError → i18n key（文案禁硬编码；code 是稳定契约）。 */

export function ModelCatalogSection({
  onCatalogChanged,
}: {
  /** 打开时的重验结束后让相邻的模型清单重取：可下载的条目可能变了（ADR-145 ①）。 */
  onCatalogChanged?: () => void;
} = {}) {
  const { t, i18n } = useTranslation("cloud");
  const [status, setStatus] = useState<CatalogStatusDto | null>(null);
  /** 能力提示词目录（ADR-050）：与模型目录同一次刷新、同一块面板，各自一行事实。 */
  const [prompts, setPrompts] = useState<PromptCatalogStatusDto | null>(null);
  const [error, setError] = useState<CloudError | null>(null);
  const mounted = useRef(true);

  const loadStatus = useCallback(async () => {
    const [result, promptResult] = await Promise.all([
      commands.catalogStatus(),
      commands.promptCatalogStatus(),
    ]);
    if (!mounted.current) return;
    if (result.status === "ok") {
      setStatus(result.data);
    } else {
      setError(result.error);
    }
    if (promptResult.status === "ok") {
      setPrompts(promptResult.data);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void loadStatus();
    // 打开即重验（ADR-170 ②）：先显示缓存的状态，Rust 判过新鲜度、需要时拉完再回读一次。
    void appCommands.revalidateCatalogs().then(() => {
      if (!mounted.current) return;
      void loadStatus();
      onCatalogChanged?.();
    });
    return () => {
      mounted.current = false;
    };
  }, [loadStatus, onCatalogChanged]);

  /** 「能力提示词」——此刻生效的是哪一版，有没有因清单落差回落内置（ADR-050）。 */
  const promptsLabel = (p: PromptCatalogStatusDto): string => {
    if (p.prompt_version === 0) {
      // 连着服务端却仍用内置：把上一次拉取的原因说出来（多半是「服务端暂无生效目录」），
      // 否则用户会以为刷新没起作用。
      return p.catalog.last_error === null
        ? t("catalog.prompts_bundled")
        : t("catalog.prompts_bundled_reason", { reason: reasonLabel(p.catalog.last_error) });
    }
    // 灰度变体（ADR-050 修订 ④）：这台设备落在哪一份，用户能对上运营的发布单。
    const variant = p.catalog.variant ? t("catalog.prompts_variant", { id: p.catalog.variant }) : "";
    if (p.missing.length > 0) {
      return t("catalog.prompts_degraded", {
        version: p.prompt_version,
        manifest: p.manifest_version ?? 0,
        missing: p.missing.length,
        variant,
      });
    }
    return t("catalog.prompts_verified", { version: p.prompt_version, variant });
  };

  /** 失败原因码 → 人话（未登记的原因码原样露出，不吞信息）。 */
  const reasonLabel = (code: string): string =>
    t(`catalog.reason.${code}`, { defaultValue: code });

  /**
   * 「当前目录」——**我现在用的是哪一份**。只看 `verified_version`，**不看 `state`**：
   * `rejected` 说的是「上一次去拿被拒了」，而此刻生效的仍是最近一份有效副本
   * （TDD-14 §3.1 的 fallback 契约）。写成「当前目录：已拒绝」是假的。
   */
  const currentLabel = (s: CatalogStatusDto): string =>
    s.verified_version !== null
      ? t("catalog.state_verified", { version: s.verified_version })
      : t("catalog.state_bundled");

  /** 「上次刷新」——**上一次去拿的结果**（时刻 + 被拒原因）。 */
  const refreshedLabel = (s: CatalogStatusDto): string => {
    if (s.state === "refreshing") return t("catalog.state_refreshing");
    if (s.last_refresh_at === null) return t("catalog.never_refreshed");
    const time = formatDateTime(s.last_refresh_at * 1000, i18n.language);
    return s.last_error === null
      ? time
      : t("catalog.refreshed_rejected", { time, reason: reasonLabel(s.last_error) });
  };

  return (
    <section className="luna-settings-panel__section" aria-label={t("catalog.title")}>
      <h2 className="luna-settings-panel__section-title">{t("catalog.title")}
        <HelpDetails summary={t("catalog.title")}>
          <p>{t("catalog.note")}</p>
        </HelpDetails>
      </h2>
      {status === null ? (
        <p className="luna-empty">…</p>
      ) : (
        <>
          {/* 两个问题、两行，各自只答自己那一个：「当前目录」= 我现在用的是哪一份；
              「上次刷新」= 上一次去拿的结果。串在一起答就必然有一个答错。 */}
          <div className="luna-settings-panel__rows">
            <FactRow label={t("catalog.current_label")} value={status.verified_version === null
              ? t("catalog.state_bundled") : t("catalog.state_verified_summary")} />
            <FactRow
              label={t("catalog.refreshed_label")}
              value={refreshedLabel(status)}
              valueRole="status"
            />
            {prompts ? (
              <FactRow label={t("catalog.prompts_label")} value={prompts.prompt_version === 0
                ? promptsLabel(prompts) : prompts.missing.length > 0
                  ? t("catalog.prompts_degraded_summary", { missing: prompts.missing.length })
                  : t("catalog.state_verified_summary")} />
            ) : null}
          </div>
          <InlineDetails summary={t("catalog.technical_details")}>
            <FactRow label={t("catalog.current_label")} value={currentLabel(status)} />
            {prompts ? <FactRow label={t("catalog.prompts_label")} value={promptsLabel(prompts)} /> : null}
            {prompts ? <p>{t("catalog.prompts_note")}</p> : null}
          </InlineDetails>
          {/* 什么时候刷新、要立刻查去哪儿：这一页不再有刷新按钮（ADR-170 ③），这一句得看得见，不收进详情。 */}
          <p className="luna-settings-panel__hint">{t("catalog.refresh_schedule")}</p>
          {error ? (
            <p className="luna-error-banner" role="alert">
              {errorMessage(error, t)}
            </p>
          ) : null}
          {status.strict_local ? (
            <p className="luna-settings-panel__hint" role="note">
              {t("catalog.strict_local_note")}
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}
