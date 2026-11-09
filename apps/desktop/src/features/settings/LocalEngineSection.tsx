/**
 * 本地推理引擎状态（随包 llama-server，ADR-143）：在跑什么、为什么有的模型没加载、熔断后的重启。
 *
 * 只在本平台有引擎包时出现：状态、导入 .gguf 文件、熔断后的重启。
 * 前端零业务（ADR-003）：加载哪些、给多大上下文、内存够不够全在 Rust 决定；这里只渲染。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  commands,
  events,
  type LocalEngineStatusDto,
} from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { FactRow } from "../../components/ui/FactRow";
import { InlineDetails } from "../../components/ui/InlineDetails";
import { HfImportSection } from "./HfImportSection";
import { noticeText } from "./model-notice";

export function LocalEngineSection({
  onModelsChanged,
  advanced = false,
}: {
  onModelsChanged?: () => void;
  /** 高级模式：另给「按 HF 仓库 id 添加」（ADR-144 ③）。 */
  advanced?: boolean;
} = {}) {
  const { t } = useTranslation("settings");
  const [status, setStatus] = useState<LocalEngineStatusDto | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const mounted = useRef(true);

  const load = useCallback(async () => {
    const result = await commands.localEngineStatus();
    if (mounted.current && result.status === "ok") setStatus(result.data);
  }, []);

  useEffect(() => {
    mounted.current = true;
    void load();
    const unlisten = events.localEngineChanged.listen(() => void load());
    return () => {
      mounted.current = false;
      void unlisten.then((stop) => stop());
    };
  }, [load]);

  const restart = useCallback(async () => {
    setRestarting(true);
    const result = await commands.localEngineRestart();
    if (!mounted.current) return;
    setRestarting(false);
    if (result.status === "ok") setStatus(result.data);
  }, []);

  const importFile = useCallback(async () => {
    setNotice(null);
    setImporting(true);
    const result = await commands.importLocalGguf();
    if (!mounted.current) return;
    setImporting(false);
    if (result.status === "error") {
      setNotice(noticeText(result.error, t));
      return;
    }
    if (result.data === null) return; // 用户取消了选择
    setNotice(
      t(result.data.matched_catalog ? "models.notice.MODEL_IMPORTED_AS_CATALOG" : "models.notice.MODEL_IMPORTED", {
        detail: result.data.name,
        role: t(`models.import_role.${result.data.role}`),
      }),
    );
    onModelsChanged?.();
  }, [onModelsChanged, t]);

  if (status === null) return null;
  return (
    <>
    <LocalEngineView
      status={status}
      restarting={restarting}
      onRestart={() => void restart()}
      importing={importing}
      onImport={() => void importFile()}
      notice={notice}
    />
    {advanced && status.gguf_enabled && status.runtime_present ? <HfImportSection onImported={onModelsChanged} /> : null}
    </>
  );
}

/** 纯展示（取数在 {@link LocalEngineSection}）；拆开是为了能被静态渲染测试直接验。 */
export function LocalEngineView({
  status,
  restarting,
  onRestart,
  importing = false,
  onImport = () => {},
  notice = null,
}: {
  status: LocalEngineStatusDto;
  restarting: boolean;
  onRestart: () => void;
  importing?: boolean;
  onImport?: () => void;
  notice?: string | null;
}) {
  const { t, i18n } = useTranslation("settings");
  // 本构建不启用 GGUF（正式构建在路由切换前）时，本地推理相关的一切都不露出。
  if (!status.gguf_enabled || !status.runtime_present) {
    return null;
  }
  const stateText =
    status.state === "restarting"
      ? t("models.local_engine.state.restarting", { count: status.failures })
      : t(`models.local_engine.state.${status.state}`);
  const bytes = (value: number | null) => formatBytes(value, i18n.language);

  return (
    <section className="luna-settings-panel__section" aria-label={t("models.local_engine.title")}>
      <h2 className="luna-settings-panel__section-title">{t("models.local_engine.title")}</h2>
      <div className="luna-settings-panel__rows">
        <FactRow
          label={t("models.local_engine.state_label")}
          value={stateText}
          valueRole="status"
        />
        {status.last_failure ? (
          <FactRow
            label={t("models.local_engine.failure_label")}
            value={t(`models.local_engine.failure.${status.last_failure}`)}
          />
        ) : null}
        {status.models.map((model) => (
          <FactRow
            key={model.model_id}
            label={t("models.local_engine.loaded_label")}
            value={t(
              model.degraded ? "models.local_engine.loaded_degraded" : "models.local_engine.loaded",
              { name: model.name, tokens: model.context_tokens },
            )}
          />
        ))}
        {status.refused.map((model) => (
          <FactRow
            key={model.model_id}
            label={t("models.local_engine.refused_label")}
            value={
              model.need_bytes === null
                ? t("models.local_engine.refused_unknown", { name: model.name })
                : t("models.local_engine.refused", {
                    name: model.name,
                    need: bytes(model.need_bytes),
                    budget: bytes(model.budget_bytes),
                  })
            }
          />
        ))}
      </div>
      {status.state === "stopped" && status.last_failure === null && status.refused.length === 0 ? (
        <p className="luna-settings-panel__hint">{t("models.local_engine.idle_hint")}</p>
      ) : null}
      {status.engine_build ? <InlineDetails summary={t("models.technical_details")}>
        <FactRow label={t("models.local_engine.build_label")} value={status.engine_build} />
      </InlineDetails> : null}
      {notice ? (
        <p className="luna-settings__notice" role="status">
          {notice}
        </p>
      ) : null}
      <div className="luna-settings-panel__actions">
        <button type="button" className="luna-btn luna-btn--secondary" disabled={importing} onClick={onImport}>
          {t(importing ? "models.local_engine.importing" : "models.local_engine.import_file")}
        </button>
        {status.state === "tripped" ? (
          <button type="button" className="luna-btn luna-btn--secondary" disabled={restarting} onClick={onRestart}>
            {t("models.local_engine.restart")}
          </button>
        ) : null}
      </div>
    </section>
  );
}
