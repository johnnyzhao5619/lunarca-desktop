// 本地模型页的正文：本机模型与运行位置。
// 候选资格、能力、就绪状态与写入校验全部来自 Rust；前端不解析模型名、路径或标签（ADR-003）。

import { useEffect, useState } from "react";
import { Pager } from "../../components/ui/Pager";
import { paginate } from "../../lib/paginate";
import { SelectBox } from "../../components/ui/SelectBox";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { useTranslation } from "react-i18next";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { InlineDetails } from "../../components/ui/InlineDetails";
import { CopyValueButton } from "../../components/ui/CopyValueButton";
import { goTo } from "../../lib/navigate";
import type {
  AsrServerGate,
  ModelAssignmentDto,
  ModelDeletePlanDto,
  ModelEntryDto,
  ModelInstallPlanDto,
  ModelSelectionMode,
  RemoteComputeDto,
} from "../../ipc/bindings";
import { cssVariables } from "../../lib/css-variables";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { ModelInstallPlanCard } from "./ModelInstallPlanCard";
import { CoremlEncoderRow, coremlProgressId } from "./CoremlEncoderRow";
import { LocalEngineSection } from "./LocalEngineSection";
import { noticeText } from "./model-notice";
import { BLOCKER_COPY, TRANSCRIPTION_CONSENT_KEY } from "./TranscriptionConsentRow";
import { ModelCatalogSection } from "./ModelCatalogSection";
import { ModelStorageRelocation } from "./ModelStorageRelocation";
import type { ModelsController } from "./use-settings";

type View = "local" | "services";

/**
 * 深链尾段 → 子视图（`settings://local_models/<section>`，TDD-07 §4.1b）。
 *
 * **fail closed**：未知尾段落回本次投影的第一个页签，绝不停在空白页。
 * `usage` 不在这里——执行记录是页上的独立面板，不是模型库的一个页签（ADR-122 ⑥）。
 *
 * 分配槽的深链形如 `ai.assignment.<slot>`（它是设置 key 而不是 section），故单列一条。
 */
export function viewForRouteTarget(target: string | null, views: View[]): View | null {
  if (target === null) return null;

  return (views as string[]).includes(target) ? (target as View) : null;
}

/** 本地模型页只负责模型清单和运行位置，模型分配归 AI 服务页。 */
const MODEL_VIEWS: View[] = ["local", "services"];

export function viewsFor(): View[] {
  return MODEL_VIEWS;
}

/**
 * 本机模型清单每页几条。行是卡片形态（标题 + 徽标 + 四五行元信息），六条已经接近一屏。
 */
const MODELS_PER_PAGE = 6;

/** 已装 = 已安装或随包（模型库入口的摘要与清单共用这一条判据）。 */
export function isInstalled(model: ModelEntryDto): boolean {
  return model.status === "installed" || model.status === "bundled";
}

function capabilityLabel(capability: string, t: (key: string) => string): string {
  return t(`models.capability.${capability}`);
}

/**
 * 把 Rust 返回的 `CODE: 细节` 通知串翻成人话（与 settings-errors 同一约定：稳定 code 在前）。
 * 未登记的 code 原样显示——宁可露出 code 也不吞掉信息。
 */

/**
 * 「服务端在替你跑这件事」的三种形态。
 *
 * 它们对用户是**同一个状态**（算力不在这台设备上），差别只在服务端怎么选执行者：
 * `managed` 按健康度选文本模型（ADR-048）、`profile` 锁定同一版向量档（ADR-048 嵌入例外）、
 * `channel` 是转写的服务端通道（ADR-038）。故名字只有一个，差别写在 note 里。
 */
export const SERVER_MANAGED_MODES: ModelSelectionMode[] = ["managed", "profile", "channel"];

export function SlotRow({
  assignment,
  assign,
  clearAssignment,
}: {
  assignment: ModelAssignmentDto;
  assign: (slot: string, targetId: string) => Promise<boolean>;
  clearAssignment: (slot: string) => Promise<boolean>;
}) {
  const { t } = useTranslation("settings");
  const candidates = assignment.candidates;
  const selected = assignment.configured_target ?? "";
  // 展示名由 Rust 下发：候选表只含**已就绪**的模型，从它反查在「有效目标尚未就绪」那一档
  // 必然落空，界面随即把 `whisper-tiny-bundled` 这类机器串摆给用户。
  //
  // **一个状态一个名字**（ADR-059 ④）：三种落点的差别（谁选模型 / 向量档锁定 / 音频通道）
  // 写在下面那句 `note` 里，不再各造一个名字。
  const serverManaged = SERVER_MANAGED_MODES.includes(assignment.selection_mode);
  const effectiveName = serverManaged ? t("models.managed.name") : assignment.effective_name;
  const managedNote = serverManaged ? t(`models.managed.note.${assignment.selection_mode}`) : null;
  const deviceCandidates = candidates.filter((candidate) => candidate.location === "device");
  const serverCandidates = candidates.filter((candidate) => candidate.location === "server");
  const label = t(`models.slot.${assignment.slot}`);
  const detail = t(`models.slot_desc.${assignment.slot}`);
  const gate = assignment.server_gate;

  return (
    // `data-setting-key` 是深链的落点：搜索命中「默认对话模型」时用户被带到这一行，
    // 而不是页面顶端。key 由 Rust 下发。
    <li
      className="luna-settings-model"
      data-setting-key={assignment.setting_key}
      tabIndex={-1}
    >
      <div className="luna-settings-model__content">
        <div className="luna-settings-model__header">
          <span className="luna-settings-model__title">{label}</span>
          <div className="luna-settings-model__badges">
            <span className="luna-settings-model__badge">
              {capabilityLabel(assignment.required_capability, t)}
            </span>
            <span className="luna-settings-model__badge" data-readiness={assignment.readiness}>
              {t(`models.readiness.${assignment.readiness}`)}
            </span>
          </div>
        </div>
        <p className="luna-settings-model__description">{detail}</p>
        {managedNote ? (
          <p className="luna-settings-model__description" role="note">
            {managedNote}
          </p>
        ) : null}
        {gate ? <ServerGateNote gate={gate} /> : null}
        <div className="luna-settings-model__meta-info">
          <span className="luna-settings-model__meta-item">
            <span className="luna-settings-model__meta-label">
              {t(effectiveLabelKey(assignment.readiness, effectiveName))}:
            </span>
            <span className="luna-settings-model__meta-value">
              {effectiveName ?? t("models.unresolved")}
            </span>
          </span>
          <span className="luna-settings-model__meta-item">
            <span className="luna-settings-model__meta-label">{t("models.source")}:</span>
            <span className="luna-settings-model__meta-value">
              {t(`models.source_kind.${assignment.source}`)}
            </span>
          </span>
        </div>
      </div>
      <div className="luna-settings-model__actions luna-settings-model__actions--assignment">
        <label className="luna-visually-hidden" htmlFor={`model-slot-${assignment.slot}`}>
          {label}
        </label>
        <SelectBox
          id={`model-slot-${assignment.slot}`}
          className="luna-settings-field__select"
          value={selected}
          onChange={(event) => {
            const target = event.target.value;
            // 空值 = 「跟随策略」这一档本身：必须真的清除显式分配，而不是当无事发生——
            // 否则它就是一个选了会弹回去的死选项。
            if (target) {
              void assign(assignment.slot, target);
            } else if (assignment.configured_target) {
              void clearAssignment(assignment.slot);
            }
          }}
        >
          {/* 空选项 = 「自动选择」，同时是清掉显式分配的**唯一**入口——旁边再放一颗
              「恢复跟随」按钮就是同一个动作两个入口（ADR-122 ③）。清掉后落到服务端托管时
              （Rust 判定 `follow_is_server_managed`）如实标成「服务端托管」，否则用户看不出
              这一档其实是「交回服务端」。 */}
          <option value="">
            {assignment.follow_is_server_managed
              ? t("models.managed.name")
              : t("models.follow_policy")}
          </option>
          {/* 用户显式配过、此刻已不在候选里（模型被删 / 能力变了）：如实留一项。 */}
          {selected && !candidates.some((candidate) => candidate.target_id === selected) ? (
            <option value={selected}>{effectiveName ?? t("models.unresolved")}</option>
          ) : null}
          {/* 服务端候选**没有模型名**：具体模型由服务端选（ADR-048），客户端不列云端目录。
              选不到时留一个灰掉的同名项：用户找的就是这个下拉，原因要在这里看得见，
              而不是只写在上方算力卡里（真机 2026-11-03）。 */}
          {serverCandidates.length > 0 ? (
            <optgroup label={t("models.location.server")}>
              {serverCandidates.map((candidate) => (
                <option key={candidate.target_id} value={candidate.target_id}>
                  {t("models.managed.name")}
                </option>
              ))}
            </optgroup>
          ) : gate ? (
            <optgroup label={t("models.location.server")}>
              <option value={SERVER_GATE_OPTION} disabled>
                {t(gate.kind === "consent_off" ? "models.managed.gate.consent_option" : "models.managed.gate.blocked_option")}
              </option>
            </optgroup>
          ) : null}
          {deviceCandidates.length > 0 ? (
            <optgroup label={t("models.location.device")}>
              {deviceCandidates.map((candidate) => (
                <option key={candidate.target_id} value={candidate.target_id}>
                  {candidate.name}
                </option>
              ))}
            </optgroup>
          ) : null}
        </SelectBox>
      </div>
    </li>
  );
}

/** 灰掉的「服务端托管」占位项的值：禁用项不会触发 onChange，它只负责被看见。 */
const SERVER_GATE_OPTION = "server-gate";

/**
 * 转写槽位选不到服务端时的那一句，外加去改那道闸的入口。同意只在开关那里给（ADR-038 ④），
 * 这里只带路、不代勾；勾不动的原因与算力卡同一份文案（`BLOCKER_COPY`）。
 */
function ServerGateNote({ gate }: { gate: AsrServerGate }) {
  const { t } = useTranslation("settings");
  const route = gate.kind === "consent_off"
    ? `settings://ai_services/${TRANSCRIPTION_CONSENT_KEY}`
    : gate.blocker === "local_only_strategy" ? "settings://ai_services/ai.route_strategy" : null;
  return (
    <p className="luna-settings-model__description" role="note">
      {gate.kind === "consent_off" ? t("models.managed.gate.consent_off") : t(BLOCKER_COPY[gate.blocker])}
      {route ? (
        <>
          {" "}
          <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => goTo(route)}>
            {t(gate.kind === "consent_off" ? "models.managed.gate.open_consent" : "models.managed.gate.change_strategy")}
          </button>
        </>
      ) : null}
    </p>
  );
}

/**
 * 这一行该说「当前生效」还是「已选，未生效」。
 *
 * **只有 `ready` 能说「当前生效」**：徽标写着「能力不匹配」而同一行写着「当前生效」，
 * 两句不可能同时为真，而用户信的是后面那句人话。下拉里**保留**那个失效选项是对的——
 * 那是用户显式配过的值。`effectiveName` 为空时不降级：那一档渲染的是「未解析」。
 */
export function effectiveLabelKey(
  readiness: string,
  effectiveName: string | null,
): "models.effective" | "models.selected_inactive" {
  if (readiness === "ready" || effectiveName === null) return "models.effective";
  return "models.selected_inactive";
}

function ManagedComputeSection({ remoteCompute }: { remoteCompute: RemoteComputeDto }) {
  const { t } = useTranslation("settings");
  return (
    <div className="luna-settings-panel__section">
      <div className="luna-settings-model__header">
        <span className="luna-settings-model__title">
          {t("models.managed.title")}
          <HelpDetails summary={t("models.managed.title")}>
            <p>{t("models.managed.intro")}</p>
          </HelpDetails>
        </span>
        <span className="luna-settings-model__badge" data-readiness="ready">
          {t("models.managed.mode")}
        </span>
      </div>
      <div className="luna-settings-model__badges">
        <span className="luna-settings-model__meta-value">{remoteCompute.endpoint}</span>
        {remoteCompute.taken_over_capabilities.map((capability) => (
          <span key={capability} className="luna-settings-model__badge">
            {capabilityLabel(capability, t)}
          </span>
        ))}
      </div>
      {remoteCompute.available_local_models > 0 ? (
        <p className="luna-settings__notice">
          {t("models.managed.local_available", { count: remoteCompute.available_local_models })}
        </p>
      ) : null}
    </div>
  );
}

export function ModelManagementPanel({
  advancedMode,
  focusKey = null,
  controller,
}: {
  advancedMode: boolean;
  /** 深链带来的目标设置 key；指向分配槽时把页签切过去（否则跳过来落在别的页）。 */
  focusKey?: string | null;
  /**
   * 由宿主（`ModelLibraryPanel`）传入，不在这里再 `useModels()` 一次：宿主本就握着同一份
   * 快照来算「已装几个」，各拉各的等于每开一次 sheet 就多一次全量 IPC 与一套进度监听。
   */
  controller: ModelsController;
}) {
  const { t, i18n } = useTranslation("settings");
  const {
    models,
    storage,
    services,
    remoteCompute,
    error,
    message,
    progressById,
    planDelete,
    startDelete,
    planInstall,
    startInstall,
    refresh,
    pauseInstall,
    coremlPhaseById,
  } = controller;
  const views = viewsFor();
  const [view, setView] = useState<View>(() => views[0]);
  // 深链落位（分配槽 / 子视图）。写成 effect 而不是初值：同一次访问里用户可能从搜索结果
  // 或启动概览连点两次，只认初值的话第二次不会动。判定在 `viewForRouteTarget` 一处。
  useEffect(() => {
    const target = viewForRouteTarget(focusKey, views);
    if (target) setView(target);
  }, [focusKey, views]);
  const activeView = views.includes(view) ? view : views[0];
  const tabs = useTablistNavigation(views, activeView, setView);
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [modelPage, setModelPage] = useState(0);
  const [actionPlan, setActionPlan] = useState<
    | { kind: "install"; plan: ModelInstallPlanDto }
    | { kind: "delete"; plan: ModelDeletePlanDto }
    | null
  >(null);

  // 槽位顺序由 Rust 的槽位表下发，前端原样渲染——在这里抄一份顺序常量就是第二份事实源。
  function renderModel(model: ModelEntryDto) {
    const installed = isInstalled(model);
    const deletable = model.status === "installed";
    const installing = installingId === model.id;
    const progress = progressById[model.id];
    const modelPlan = actionPlan?.plan.model_id === model.id ? actionPlan : null;
    const total = progress?.total_bytes ?? 0;
    const percent =
      progress && total > 0
        ? Math.min(100, Math.round(((progress.downloaded_bytes ?? 0) / total) * 100))
        : null;

    return (
      <li key={model.id} className="luna-settings-model" data-status={model.status}>
        <div className="luna-settings-model__content">
          <div className="luna-settings-model__header">
            <span className="luna-settings-model__title">{model.name}</span>
            <div className="luna-settings-model__badges">
              <span className="luna-settings-model__badge">
                {t(`models.status.${model.status}`)}
              </span>
              {/* 有更新只提示、不替换（ADR-145 ④）：用户点「更新」走同一张确认卡。 */}
              {model.update_size_bytes !== null ? (
                <span className="luna-settings-model__badge" data-update="available">
                  {t("models.update_available")}
                </span>
              ) : null}
              {/* License 归高级投影（PRD-09-072）：它回答合规问题，而「我该装哪个」只需要
                  知道装没装、能干什么、多大。 */}
              {advancedMode && model.license_label ? (
                <span className="luna-settings-model__badge">{model.license_label}</span>
              ) : null}
              {model.effective_capabilities.map((capability) => (
                <span key={capability} className="luna-settings-model__badge">
                  {capabilityLabel(capability, t)}
                </span>
              ))}
            </div>
          </div>
          <div className="luna-settings-model__meta-info">
            <span className="luna-settings-model__meta-item">
              <span className="luna-settings-model__meta-label">
                {t(model.on_disk_bytes === null ? "models.estimated_on_disk" : "models.on_disk")}:
              </span>
              <span className="luna-settings-model__meta-value">
                {formatBytes(model.on_disk_bytes ?? model.est_on_disk_bytes, i18n.language)}
              </span>
            </span>
            {model.assigned_slots.length > 0 ? (
              <span className="luna-settings-model__meta-item">
                <span className="luna-settings-model__meta-label">{t("models.assigned")}:</span>
                <span className="luna-settings-model__meta-value">
                  {model.assigned_slots.map((slot) => t(`models.slot.${slot}`)).join(" · ")}
                </span>
              </span>
            ) : null}
            {/* 说话人分割**没有分配槽位，这是设计而不是缺口**（ADR-059 ③）：用哪一档取决于
                这场会议说的是什么语言，而那要到执行那一刻才知道。但「装了却看不出谁在用」
                同样不行，故如实说明它按什么选。 */}
            {model.effective_capabilities.includes("diarize") ? (
              <span className="luna-settings-model__meta-item">
                <span className="luna-settings-model__meta-label">{t("models.assigned")}:</span>
                <span className="luna-settings-model__meta-value">{t("models.diarize_auto")}</span>
              </span>
            ) : null}
            <span className="luna-settings-model__meta-item">
              <span className="luna-settings-model__meta-label">
                {t("models.device_fit_label")}:
              </span>
              <span className="luna-settings-model__meta-value">
                {t(`models.device_fit.${model.device_fit}`)}
              </span>
            </span>
          </div>
          {advancedMode ? <InlineDetails summary={t("models.technical_details_for", { name: model.name })}>
            <div className="luna-settings-model__meta-info">
            {/* 来源、量化与摘要用于排障，主要状态和设备适配始终留在清单首层。 */}
            {advancedMode ? (
              <span className="luna-settings-model__meta-item">
                <span className="luna-settings-model__meta-label">
                  {t("models.manager_label")}:
                </span>
                <span className="luna-settings-model__meta-value">
                  {t(`models.manager.${model.managed_by}`)}
                </span>
              </span>
            ) : null}
            {advancedMode && model.quantization ? (
              <span className="luna-settings-model__meta-item">
                <span className="luna-settings-model__meta-label">{t("models.quantization")}:</span>
                <span className="luna-settings-model__meta-value">{model.quantization}</span>
              </span>
            ) : null}
            {advancedMode && model.estimated_memory_bytes !== null ? (
              <span className="luna-settings-model__meta-item">
                <span className="luna-settings-model__meta-label">
                  {t("models.memory_estimate")}:
                </span>
                <span className="luna-settings-model__meta-value">
                  {formatBytes(model.estimated_memory_bytes, i18n.language)}
                </span>
              </span>
            ) : null}
            {advancedMode && model.runtime_digest ? (
              <span className="luna-settings-model__meta-item">
                <span className="luna-settings-model__meta-label">{t("models.digest")}:</span>
                <span className="luna-settings-model__meta-value">
                  {model.runtime_digest}
                  <CopyValueButton value={model.runtime_digest} label={t("models.copy_digest")} />
                </span>
              </span>
            ) : null}
            {advancedMode ? (
              <span className="luna-settings-model__meta-item">
                <span className="luna-settings-model__meta-label">{t("models.origin_label")}:</span>
                <span className="luna-settings-model__meta-value">
                  {t(`models.origin.${model.origin}`)}
                </span>
              </span>
            ) : null}
            </div>
          </InlineDetails> : null}
          {/* CoreML 加速行（ADR-115）：只有编入了 CoreML 且目录里登记了编码器的 whisper 档才出现。 */}
          {advancedMode && model.coreml_available ? (
            <CoremlEncoderRow
              model={model}
              progress={progressById[coremlProgressId(model.id)]}
              phase={coremlPhaseById[model.id]}
              models={controller}
            />
          ) : null}
          {percent !== null && !progress.done ? (
            <div className="luna-settings-model__progress-container">
              <div className="luna-settings-model__progress-label">
                <span>
                  {t("models.progress")}: {percent}%
                </span>
                {/* 暂停（PRD-14-020）：断点留在暂存目录，再点「下载」从断点续。 */}
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => void pauseInstall(model.id)}
                >
                  {t("models.pause")}
                </button>
              </div>
              <div className="luna-settings-model__progress-bar">
                <div
                  className="luna-settings-model__progress-fill"
                  style={cssVariables({ "--progress-inline-size": `${percent}%` })}
                />
              </div>
            </div>
          ) : null}
          {modelPlan?.kind === "install" ? (
            /* 安装确认卡与首启引导同一张（`ModelInstallPlanCard`，PRD-14-020）。 */
            <ModelInstallPlanCard
              plan={modelPlan.plan}
              busy={installing}
              onConfirm={() => {
                // 确认即收卡：`startInstall` 要等整段下载跑完才返回，等它再收的话确认卡会
                // 在进度条旁边一直挂到下载结束。开始失败时错误经 controller 的 error 呈现。
                setActionPlan(null);
                setInstallingId(model.id);
                void startInstall(modelPlan.plan.plan_id).finally(() => setInstallingId(null));
              }}
              onCancel={() => setActionPlan(null)}
            />
          ) : modelPlan?.kind === "delete" ? (
            <div
              className="luna-settings-model__plan"
              role="group"
              aria-label={t("models.plan.delete_title")}
            >
              <strong>{t("models.plan.delete_title")}</strong>
              <span>
                {t("models.plan.release", { value: formatBytes(modelPlan.plan.release_bytes, i18n.language) })}
              </span>
              <span>{t("models.plan.permanent")}</span>
              {modelPlan.plan.blocking_slots.length > 0 ? (
                <span>
                  {t("models.plan.blocking_slots", {
                    value: modelPlan.plan.blocking_slots
                      .map((slot) => t(`models.slot.${slot}`))
                      .join(" · "),
                  })}
                </span>
              ) : null}
              {modelPlan.plan.blocking_reason ? (
                <span className="luna-field-error" role="alert">
                  {t(`models.plan.reason.${modelPlan.plan.blocking_reason}`)}
                </span>
              ) : null}
              <div className="luna-settings-model__actions">
                <button
                  type="button"
                  className="luna-btn luna-btn--primary"
                  disabled={!modelPlan.plan.can_delete}
                  onClick={() => {
                    void startDelete(modelPlan.plan.plan_id).then((deleted) => {
                      if (deleted) setActionPlan(null);
                    });
                  }}
                >
                  {t("models.plan.delete_confirm")}
                </button>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => setActionPlan(null)}
                >
                  {t("models.plan.cancel")}
                </button>
              </div>
            </div>
          ) : null}
        </div>
        {/* **只渲染做得了的那一颗**。随包模型既删不掉也不用装，此前两颗按钮一起灰着——
            一个永远点不动的按钮不是禁用状态，是噪声（真机：Whisper Tiny 那一行两颗全灰）。
            「装不了」这件事由状态徽标说（`models.status.unavailable`），不由灰按钮说。 */}
        <div className="luna-settings-model__actions">
          {deletable ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={modelPlan !== null}
              onClick={() =>
                void planDelete(model.id).then((plan) => {
                  if (plan) setActionPlan({ kind: "delete", plan });
                })
              }
            >
              {t("models.delete")}
            </button>
          ) : null}
          {installed && model.update_size_bytes !== null ? (
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={installing || modelPlan !== null}
              onClick={() => {
                void planInstall(model.id).then((plan) => {
                  if (plan) setActionPlan({ kind: "install", plan });
                });
              }}
            >
              {installing
                ? t("models.installing")
                : t("models.update", {
                    value: formatBytes(model.update_size_bytes, i18n.language),
                  })}
            </button>
          ) : null}
          {!installed && model.status !== "unavailable" ? (
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={installing || modelPlan !== null}
              onClick={() => {
                void planInstall(model.id).then((plan) => {
                  if (plan) setActionPlan({ kind: "install", plan });
                });
              }}
            >
              {installing
                ? t("models.installing")
                : model.partial_bytes !== null
                  ? t("models.resume_from", {
                      value: formatBytes(model.partial_bytes, i18n.language),
                    })
                  : t("models.install")}
            </button>
          ) : null}
        </div>
      </li>
    );
  }

  return (
    <section
      id="settings-section-models"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("models.title")}
    >
      {/* **不贴自己的标题**：这一面独占整张 sheet，标题与存储占用由 sheet 头栏给
          （PRD-09-074，再写一遍就是同屏重复）。 */}
      {/* 分段形态而不是三颗并排的按钮：`luna-btn--secondary` 与 sheet 右上角那颗「完成」
          同形，选中态只靠边框色，用户读不出这是三选一。 */}
      <div className="luna-settings-panel__actions">
      <div className="luna-settings-models__tabs" role="tablist" aria-label={t("models.title")}>
        {views.map((item) => (
          <button
            key={item}
            ref={tabs.register(item)}
            id={`model-tab-${item}`}
            type="button"
            role="tab"
            aria-controls={`settings-section-${item}`}
            aria-selected={activeView === item}
            tabIndex={tabs.tabIndexOf(item)}
            className="luna-settings-models__tab"
            onClick={() => setView(item)}
            onKeyDown={tabs.onKeyDown}
          >
            {t(`models.view.${item}`)}
          </button>
        ))}
      </div>
      {activeView === "local" && remoteCompute ? <button type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => goTo("settings://ai_services/assignments")}>
        {t("models.execution_policy")}
      </button> : null}
      </div>
      {error ? (
        <p className="luna-error-banner" role="alert">
          {error}
        </p>
      ) : null}
      {message ? <p className="luna-settings__notice">{noticeText(message, t)}</p> : null}
      {activeView === "local" ? (
        <section
          id="settings-section-local"
          tabIndex={-1}
          role="tabpanel"
          aria-labelledby="model-tab-local"
          className="luna-settings-panel__section"
        >
          {models.length === 0 ? (
            <p className="luna-empty">{t("models.empty")}</p>
          ) : (
            <>
              {/* 清单在卡片里铺开（ADR-126 第 7 条），故长度由分页封顶：目录里十几条
                  模型、每条三四行，全铺会把同页另外三张卡挤到屏幕之外。 */}
              <ul className="luna-settings-models__list">
                {paginate(models, modelPage, MODELS_PER_PAGE).items.map(renderModel)}
              </ul>
              <Pager
                page={paginate(models, modelPage, MODELS_PER_PAGE).page}
                pageCount={paginate(models, modelPage, MODELS_PER_PAGE).pageCount}
                onPageChange={setModelPage}
                label={t("models.view.local")}
              />
            </>
          )}
          {/* 引擎状态紧跟清单：装完一个本地文本模型，下面就能看到它被加载（或为什么没加载）。 */}
          <LocalEngineSection onModelsChanged={refresh} advanced={advancedMode} />
          {/* 先给可用模型与安装动作，目录状态和刷新仍紧邻同一份清单。 */}
          <ModelCatalogSection onCatalogChanged={refresh} />
          {storage ? (
            <ModelStorageRelocation modelsRoot={storage.models_root} onRelocated={refresh} />
          ) : null}
        </section>
      ) : null}
      {activeView === "services" ? (
        <section
          id="settings-section-services"
          tabIndex={-1}
          role="tabpanel"
          aria-labelledby="model-tab-services"
          className="luna-settings-panel__section"
        >
          {remoteCompute ? <ManagedComputeSection remoteCompute={remoteCompute} /> : null}
          {advancedMode || !remoteCompute ? (
            <>
              <ul className="luna-settings-models__list">
                {services.map((service) => (
                  <li key={service.id} className="luna-settings-model">
                    <div className="luna-settings-model__content">
                      <div className="luna-settings-model__header">
                        <span className="luna-settings-model__title">{service.name}</span>
                        <span className="luna-settings-model__badge" data-readiness={service.status}>
                          {t(`models.service.status.${service.status}`)}
                        </span>
                      </div>
                      <p className="luna-settings-model__description">
                        {t(`models.service.${service.id}_body`)}
                      </p>
                      <div className="luna-settings-model__meta-info">
                        <span className="luna-settings-model__meta-item">
                          <span className="luna-settings-model__meta-label">
                            {t("models.manager_label")}:
                          </span>
                          <span className="luna-settings-model__meta-value">
                            {t(`models.manager.${service.managed_by}`)}
                          </span>
                        </span>
                        <span className="luna-settings-model__meta-item">
                          {t("models.service.model_count", { count: service.model_count })}
                        </span>
                        {service.endpoint ? (
                          <span className="luna-settings-model__meta-item">
                            <span className="luna-settings-model__meta-label">
                              {t("models.service.endpoint")}:
                            </span>
                            <span className="luna-settings-model__meta-value">
                              {service.endpoint}
                            </span>
                          </span>
                        ) : (
                          <span className="luna-settings-model__meta-item">
                            {t("models.service.in_process")}
                          </span>
                        )}
                        {service.latency_ms !== null ? (
                          <span className="luna-settings-model__meta-item">
                            {t("models.service.latency", { value: service.latency_ms })}
                          </span>
                        ) : null}
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
              {/* **一颗刷新，不是每行一颗**（ADR-122 ③ 同一条）：逐行那几颗点下去跑的是同一个
                  全量重探，读起来却像可以单独探某一个。 */}
              <div className="luna-settings-panel__actions">
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => void refresh()}
                >
                  {t("models.service.refresh")}
                </button>
              </div>
            </>
          ) : null}
        </section>
      ) : null}
    </section>
  );
}
