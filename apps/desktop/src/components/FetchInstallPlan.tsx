import { useTranslation } from "react-i18next";
import type { FetchInstallPlanDto } from "../ipc/bindings";
import { formatBytes } from "../lib/format-bytes";
import { FactRow } from "./ui/FactRow";
import { HelpDetails } from "./ui/HelpDetails";
import { PathDetails } from "./ui/PathDetails";

export interface FetchInstallPlanProps {
  /** Rust 给出的计划（`null` = 尚未取回 → 不渲染，不先摆一个猜出来的清单）。 */
  plan: FetchInstallPlanDto | null;
}

/**
 * 「安装下载工具」在点击之前的计划——「从链接导入」与「外部工具」面板共用的唯一呈现
 * （PRD-05-007 2026-10-08 修订）。
 *
 * 它替代的是一段固定说明（"会从官方仓库下载 yt-dlp，约 38 MB；如果本机一个 JS 运行时都没有，
 * 会一并装上 Deno……"）：那段话要用户自己判断"我这台机器属于哪种情况"，而计划直接列出
 * **这台机器**将装的几件、来源、约多大、装到哪。体积是约数（计划阶段不联网），精确值由
 * 下载进度给出；组件取舍与执行共用 Rust 的同一个判定。
 */
export function FetchInstallPlan({ plan }: FetchInstallPlanProps) {
  const { t, i18n } = useTranslation("transcribe");
  if (plan === null) {
    return null;
  }
  return (
    <div className="luna-fact-list" aria-label={t("fetch.plan.title")} role="group">
      {plan.components.map((part) => {
        const download = part.approx_download_bytes ?? null;
        const disk = part.approx_disk_bytes ?? null;
        const size =
          download === null
            ? t("fetch.plan.sizeUnknown")
            : disk !== null && disk !== download
              ? t("fetch.plan.sizeUnpacked", {
                  download: formatBytes(download, i18n.language),
                  disk: formatBytes(disk, i18n.language),
                })
              : t("fetch.plan.size", { size: formatBytes(download, i18n.language) });
        return (
          <FactRow
            key={part.tool}
            label={t(
              part.replaces_managed
                ? `fetch.plan.component.${part.tool}_replace`
                : `fetch.plan.component.${part.tool}`,
            )}
            hint={t("fetch.plan.source", { source: part.source })}
            value={size}
          />
        );
      })}
      <FactRow label={t("fetch.plan.location")} hint={t("fetch.plan.permission")} value={<>
        {t("fetch.plan.location_summary")}
        <HelpDetails summary={t("fetch.plan.scope_title")}><p>{t("fetch.plan.scope")}</p></HelpDetails>
      </>} />
      <div className="luna-fact-list__note">
        <PathDetails path={plan.install_dir} preview={false}><p>{t("fetch.plan.note")}</p></PathDetails>
      </div>
    </div>
  );
}
