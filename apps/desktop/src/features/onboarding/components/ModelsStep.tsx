// 模型准备（PRD-14-010/020/021/022/023）：顶部一行设备算力摘要（芯片 · 内存 · 档位），
// 按档位推荐的 whisper 与本机文本模型各一张卡，下载都走与设置 › 模型页**同一张**确认卡
// （体积 / 装机占用 / 剩余 / 许可 / 拦截原因），离线时不出下载按钮。文本模型由随包推理
// 引擎运行（ADR-143），用户不必另装任何运行时。安装/拦截/推荐逻辑全在 Rust——文本模型与 whisper 下载
// 在 Rust 侧是同一条 plan → start 路径，进度也走同一个事件。

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CpuIcon, DownloadIcon } from "@lunarca/editor-core/Icon";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { cssVariables } from "../../../lib/css-variables";
import {
  type ModelDownloadProgress,
  type ModelEntryDto,
  type ModelInstallPlanDto,
  type SetupStatusDto,
} from "../../../ipc/bindings";
import { ModelInstallPlanCard } from "../../settings/ModelInstallPlanCard";
import { CoremlEncoderRow, coremlProgressId } from "../../settings/CoremlEncoderRow";
import type { useModels } from "../../settings/use-settings";
import type { HardwareProbe } from "../use-hardware";

type ModelsController = ReturnType<typeof useModels>;

/**
 * 一张「推荐模型」卡：名称 + 角色/许可徽标 + 体积 + 进度 + 就绪/离线/下载三态动作；
 * 点下载先出确认卡（PRD-14-020）。whisper 与本机文本模型共用——两处各画一张的话，
 * 迟早一边少一行披露。
 *
 * `role` 是**这一档在当前算力落点下的角色**，不是两种皮肤：服务端形态下本机 whisper
 * 只是断线兜底（ADR-038 ⑤），把它摆成一颗主按钮加「推荐」徽标，等于在刚承诺过零下载的
 * 那条路上再催一次下载。角色变的是徽标与按钮分量，披露与确认流一个字不改。
 */
function RecommendedModelCard({
  model,
  progress,
  online,
  models,
  role = "recommended",
}: {
  model: ModelEntryDto;
  progress: ModelDownloadProgress | undefined;
  online: boolean;
  models: ModelsController;
  role?: "recommended" | "fallback";
}) {
  const { t, i18n } = useTranslation("onboarding");
  const [plan, setPlan] = useState<ModelInstallPlanDto | null>(null);
  const [installing, setInstalling] = useState(false);
  const installed = model.status === "installed" || model.status === "bundled";
  const downloading = progress !== undefined && !progress.done;
  const percent =
    progress && !progress.done && (progress.total_bytes ?? 0) > 0
      ? Math.min(
          100,
          Math.round(((progress.downloaded_bytes ?? 0) / (progress.total_bytes ?? 1)) * 100),
        )
      : null;
  // CoreML 加速讲的是**这一档权重**的事，不是页面上第二件事：它收进卡内的展开块，
  // 与算力落点选中后的连接表单同一形态。此前它是一行没有卡的裸控件，紧贴在模型卡下面
  // ——同一页里一张卡加一条裸行，那条行读起来像掉在外面的（真机 2026-09-12 截图）。
  const coreml = installed && model.coreml_available;

  return (
    <>
      <div
        className="luna-onboarding-item"
        data-layout="stack"
        data-align={downloading ? undefined : "center"}
      >
        <div className="luna-onboarding-item__main">
        {/* 图标与权限行、任务卡同一个 32px 盒：已就绪转成功语气。此前模型卡是全引导唯一
            一张没有图标的卡，三张卡并排时它的标题起点比别的靠左一截。
            图标**不随状态换形**：就绪时画一枚 ✓ 会读成"一个可以取消勾选的复选框"（真机
            2026-09-12 截图），而"装没装"右边那枚徽标已经说了。 */}
        <span
          className="luna-onboarding-item__icon"
          data-tone={installed ? "success" : undefined}
          aria-hidden="true"
        >
          <CpuIcon size={18} />
        </span>
        <div className="luna-onboarding-item__text">
          <div className="luna-onboarding-model__title">
            <strong>{model.name}</strong>
            <span className="luna-onboarding__badge">
              {t(role === "fallback" ? "step.models.fallback_badge" : "step.models.recommended_badge")}
            </span>
            {model.license_label ? (
              <span className="luna-onboarding__badge" data-tone="muted">
                {model.license_label}
              </span>
            ) : null}
          </div>
          {model.size_bytes !== null ? (
            <span className="luna-onboarding-model__meta">
              {t(model.status === "bundled" ? "step.models.size_bundled" : "step.models.size_note", {
                size: formatBytes(model.size_bytes, i18n.language),
              })}
            </span>
          ) : null}
          {downloading ? (
            <div className="luna-onboarding-model__progress">
              <div className="luna-onboarding-model__progress-bar">
                <div
                  className="luna-onboarding-model__progress-fill"
                  data-indeterminate={percent === null}
                  style={
                    percent === null
                      ? undefined
                      : cssVariables({ "--progress-inline-size": `${percent}%` })
                  }
                />
              </div>
              <span>
                {t("step.models.progress", {
                  done: formatBytes(progress?.downloaded_bytes ?? 0, i18n.language),
                  total: formatBytes(progress?.total_bytes ?? null, i18n.language),
                })}
              </span>
              {/* 暂停（PRD-14-020）：断点留在暂存目录，再点「下载」从断点续。 */}
              <button
                type="button"
                className="luna-onboarding__inline-action"
                onClick={() => void models.pauseInstall(model.id)}
              >
                {t("step.models.pause")}
              </button>
            </div>
          ) : null}
        </div>
        <div className="luna-onboarding-item__action">
          {installed ? (
            <span className="luna-onboarding__badge">{t("step.models.installed")}</span>
          ) : !online ? (
            <span className="luna-onboarding-model__meta" role="status">
              {t("step.models.offline")}
            </span>
          ) : (
            <button
              type="button"
              className={
                role === "fallback" ? "luna-btn luna-btn--secondary" : "luna-btn luna-btn--primary"
              }
              disabled={installing || downloading || plan !== null}
              onClick={() => {
                void models.planInstall(model.id).then((next) => {
                  if (next) setPlan(next);
                });
              }}
            >
              <DownloadIcon size={14} />
              {installing || downloading
                ? t("step.models.installing")
                : model.partial_bytes !== null
                  ? t("step.models.resume_from", {
                      value: formatBytes(model.partial_bytes, i18n.language),
                    })
                  : t("step.models.install")}
            </button>
          )}
          </div>
        </div>

        {/* CoreML 加速（ADR-115）：与设置页同一组件，只换了落点。 */}
        {coreml ? (
          <div className="luna-onboarding-item__detail">
            <CoremlEncoderRow
              model={model}
              progress={models.progressById[coremlProgressId(model.id)]}
              phase={models.coremlPhaseById[model.id]}
              models={models}
              compact
            />
          </div>
        ) : null}
      </div>

      {/* 与设置页同一张确认卡（PRD-14-020）：体积、装机占用、剩余空间、许可、拦截原因。 */}
      {plan ? (
        <ModelInstallPlanCard
          plan={plan}
          busy={installing}
          onConfirm={() => {
            // 确认即收卡（理由同设置页）：`startInstall` 等整段下载跑完才返回。
            setPlan(null);
            setInstalling(true);
            void models.startInstall(plan.plan_id).finally(() => setInstalling(false));
          }}
          onCancel={() => setPlan(null)}
        />
      ) : null}
    </>
  );
}

export function ModelsStep({
  hardware,
  models,
  setup,
}: {
  hardware: HardwareProbe;
  models: ModelsController;
  setup: SetupStatusDto | null;
}) {
  const { t, i18n } = useTranslation("onboarding");
  const { models: entries, error, progressById } = models;
  const [online, setOnline] = useState<boolean>(() =>
    typeof navigator === "undefined" ? true : navigator.onLine,
  );

  // 离线提示只看浏览器给的连通性（不主动探测任何地址——PRD-14-022 的联网检测不该自己
  // 制造一次联网）；上线/离线即时切换。
  useEffect(() => {
    const on = () => setOnline(true);
    const off = () => setOnline(false);
    window.addEventListener("online", on);
    window.addEventListener("offline", off);
    return () => {
      window.removeEventListener("online", on);
      window.removeEventListener("offline", off);
    };
  }, []);

  const asrModels = entries.filter((model) => model.effective_capabilities.includes("transcribe"));
  // 只数**已安装**的：目录里的推荐项状态是 downloadable，此前一并数进去，
  // 于是一个模型都没拉过的机器也被告知「N 个模型可直接复用」。
  const installedLlms = entries.filter(
    (model) => model.effective_capabilities.includes("chat") && model.status === "installed",
  );
  const bundledReady = asrModels.some((model) => model.status === "bundled");

  const recommendedAsrId = hardware.summary?.recommended_asr_model_id ?? null;
  const recommendedAsr = recommendedAsrId
    ? (asrModels.find((model) => model.id === recommendedAsrId) ?? null)
    : null;
  const recommendedLlmId = hardware.summary?.recommended_llm_model ?? null;
  const recommendedLlm = recommendedLlmId
    ? (entries.find((model) => model.id === recommendedLlmId) ?? null)
    : null;

  const serverCompute =
    setup?.compute_location === "official_cloud" || setup?.compute_location === "organization";
  // 「下载在后台继续」只在**确实有下载在跑**时说；还没开始时说它，是提前宣告一件没发生的事
  // （2026-10-08 文案与交互审查 C07）。
  const anyDownloading = Object.values(progressById).some(
    (progress) => progress !== undefined && !progress.done,
  );
  const asrHint = serverCompute
    ? "step.models.fallback_hint"
    : anyDownloading
      ? "step.models.later_hint_downloading"
      : bundledReady
        ? null
        : "step.models.later_hint";

  return (
    <div className="luna-onboarding-step">
      {/* 设备算力摘要一行（PRD-14-010）：此前独占一步，而它没有任何用户决策。 */}
      {hardware.loading ? (
        <p className="luna-onboarding__pending">{t("step.models.hardware_checking")}</p>
      ) : hardware.summary ? (
        <div className="luna-onboarding-hardware__tier">
          <span>
            {t("step.models.hardware_line", {
              chip: hardware.summary.chip,
              memory: formatBytes(hardware.summary.memory_bytes, i18n.language),
            })}
          </span>
          <span
            className="luna-onboarding__badge"
            data-tone={hardware.summary.tier === "low" ? "warning" : undefined}
          >
            {t(`step.models.tier.${hardware.summary.tier}`)}
          </span>
        </div>
      ) : (
        <p className="luna-onboarding__error" role="alert">
          {t("step.models.hardware_error")}
        </p>
      )}
      {hardware.summary?.tier === "low" ? (
        <p className="luna-onboarding__hint">{t("step.models.low_hint")}</p>
      ) : null}

      {/*
        算力在服务端时，本机 whisper 是**断线兜底**而不是这条路的主力：标题、徽标、按钮
        分量与结语都按这个角色说话，就绪与否如实标注。三态那边同样不再把它当作本步的
        前置（`derive_steps`），否则引导第三步刚说完「零下载」，第四步就把下载列进待办。
      */}
      <section className="luna-onboarding-models__section">
        <h3>{t(serverCompute ? "step.models.asr_fallback_title" : "step.models.asr_title")}</h3>
        {bundledReady ? (
          <p className="luna-onboarding__ready">{t("step.models.bundled_ready")}</p>
        ) : serverCompute ? (
          <p className="luna-onboarding__pending">{t("step.models.fallback_not_ready")}</p>
        ) : null}
        {recommendedAsr ? (
          <RecommendedModelCard
            model={recommendedAsr}
            progress={progressById[recommendedAsr.id]}
            online={online}
            models={models}
            role={serverCompute ? "fallback" : "recommended"}
          />
        ) : null}
        {asrHint !== null ? <p className="luna-onboarding__hint">{t(asrHint)}</p> : null}
      </section>

      {/*
        算力在服务端时**整段不出现**：文本优化与翻译已经可用，而这一段会催用户去下一个
        他用不上的本机模型。判据取 Rust 的落点。本机文本模型由随包推理引擎运行（ADR-143），
        不再需要用户另装任何东西——推荐档就地一键下载，与转写模型同一张确认卡。
      */}
      {!serverCompute && hardware.summary ? (
        <section className="luna-onboarding-models__section">
          <h3>{t("step.models.llm_title")}</h3>
          {installedLlms.length > 0 ? (
            <p className="luna-onboarding__ready">
              {t("step.models.llm_detected", { count: installedLlms.length })}
            </p>
          ) : (
            <p className="luna-onboarding__pending">{t("step.models.llm_none")}</p>
          )}
          {recommendedLlm ? (
            <RecommendedModelCard
              model={recommendedLlm}
              progress={progressById[recommendedLlm.id]}
              online={online}
              models={models}
            />
          ) : null}
        </section>
      ) : null}

      {error ? (
        <p className="luna-onboarding__error" role="alert">
          {t("step.models.error")}: {error}
        </p>
      ) : null}
    </div>
  );
}
