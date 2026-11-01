// 发行溯源卡片（TDD-13 §4）：字段全由 Rust 的 app_info 下发，前端只呈现。
// 同卡附「上一次启动」时间线（TDD-13 §6 冷启动指标，ADR-114 ①）：里程碑与毫秒数全由
// Rust 记，前端只把它们排成行。
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type AppInfo,
  type StartupDiagnosticsDto,
  type StartupMilestone,
} from "../../ipc/bindings";

/** 呈现顺序 = 启动顺序；缺席的里程碑不渲染（上一次启动可能没走到）。 */
const STARTUP_MILESTONES: readonly StartupMilestone[] = [
  "shortcuts_registered",
  "setup_end",
  "first_frame",
  "workspace_ready",
  "engine_ready",
];

export function BuildInfoPanel() {
  const { t } = useTranslation("settings");
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [startup, setStartup] = useState<StartupDiagnosticsDto | null>(null);

  useEffect(() => {
    let disposed = false;
    void commands
      .appInfo()
      .then((next) => {
        if (!disposed) setInfo(next);
      })
      .catch(() => {
        // 应用正在退出时 IPC 可能已不可用；只读信息不应制造用户可行动的错误提示。
      });
    void commands
      .startupDiagnostics()
      .then((next) => {
        if (!disposed) setStartup(next);
      })
      .catch(() => {
        // 同上：只读诊断，取不到就不渲染那一段。
      });
    return () => {
      disposed = true;
    };
  }, []);

  if (info === null) return null;
  const previous = startup?.previous ?? null;

  return (
    <section className="luna-settings-panel" aria-labelledby="build-info-title">
      <header className="luna-settings-panel__header">
        <div>
          <h2 id="build-info-title" className="luna-settings-panel__section-title">
            {t("build_info.title")}
          </h2>
          <p className="luna-settings-panel__desc">{t("build_info.description")}</p>
        </div>
      </header>
      <div className="luna-settings-panel__rows">
        <BuildInfoRow label={t("build_info.version")} value={info.version} />
        <BuildInfoRow label={t("build_info.git_sha")} value={info.git_sha} />
        <BuildInfoRow label={t("build_info.protocol_version")} value={String(info.protocol_version)} />
        <BuildInfoRow label={t("build_info.build_time")} value={info.build_time} />
      </div>
      {previous ? (
        <>
          <p className="luna-settings-panel__desc">{t("build_info.startup_previous")}</p>
          <div className="luna-settings-panel__rows">
            {STARTUP_MILESTONES.map((milestone) => {
              const mark = previous.find((item) => item.milestone === milestone);
              return mark ? (
                <BuildInfoRow
                  key={milestone}
                  label={t(`build_info.startup_milestone.${milestone}`)}
                  value={t("build_info.startup_ms", { value: mark.elapsed_ms })}
                />
              ) : null;
            })}
          </div>
        </>
      ) : null}
    </section>
  );
}

function BuildInfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="luna-settings-panel__row">
      <span className="luna-settings-panel__row-title">{label}</span>
      <code className="luna-settings-panel__row-value">{value}</code>
    </div>
  );
}
