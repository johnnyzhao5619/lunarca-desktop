// 第一个任务（PRD-14-050/051/052）：就地试一次语音输入或弹窗翻译；成功由 Rust 判定
// （`onboarding.first_task_done`，语音落地 / 弹窗出译文即写），这里只读 `setup_status`
// 切到成功态；成功态兼作收尾：算力落点 + 仍未完成的项（快捷键自成一步，见 ShortcutsStep）。
// 麦克风在这里按需申请（Apple HIG：在用到的语境里申请），而不是在权限页一次要齐。
//
// **能力未就绪时不摆一颗必然失败的按钮**（PRD-14-051）：引擎没准备好就把「试一次」
// 换成「去准备模型」并跳回上一步。这是激活率命门上唯一一颗点了会失败的键——失败信息
// 还只是一句「呼出失败」，而用户此刻正在判断这个产品能不能用。
//
// 第三张卡（转写随包示例）补的是一条真实的死胡同：拒绝了麦克风、又没有本机语言模型的
// 用户，前两张卡同时是灰的，而这是漏斗最后一环。它不要权限、不要下载，走的是与「拖入一个
// 音频文件」同一条导入路径，成功同样由用量回填判定——不给这条路另开一个标记。

import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { commands, type SetupStatusDto } from "../../../ipc/bindings";
import { CheckIcon, LanguagesIcon, MicIcon, MusicIcon } from "@lunarca/editor-core/Icon";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import { useSettings } from "../../settings/use-settings";
import { vaultErrorText } from "../../../lib/vault-error";
import type { OnboardingStepId } from "../onboarding-steps";

/**
 * 一张「试一次」卡。三条路（语音输入 / 弹窗翻译 / 转写随包示例）共用——各画一份的话，
 * "引擎没就绪时该怎么呈现"迟早只在其中一张上成立。
 */
function TaskCard({
  icon,
  title,
  desc,
  ready,
  notReady,
  tryLabel,
  tone,
  onTry,
  onPrepare,
  children,
}: {
  icon: ReactNode;
  title: string;
  desc: string;
  ready: boolean;
  /** 引擎没就绪时替换正文的那句话。 */
  notReady: string;
  tryLabel: string;
  tone: "primary" | "secondary";
  onTry: () => void;
  /** 去「模型准备」把它备齐。 */
  onPrepare: () => void;
  children?: ReactNode;
}) {
  const { t } = useTranslation("onboarding");
  return (
    <div className="luna-onboarding-item" data-align={children ? undefined : "center"}>
      {/* 图标语气就是这张卡此刻能不能用：没就绪转静音，与权限行「还没到点上」同一档。 */}
      <span
        className="luna-onboarding-item__icon"
        data-tone={ready ? undefined : "muted"}
        aria-hidden="true"
      >
        {icon}
      </span>
      <div className="luna-onboarding-item__text">
        <strong>{title}</strong>
        <p>{ready ? desc : notReady}</p>
        {children}
      </div>
      <div className="luna-onboarding-item__action">
        {ready ? (
          <button
            type="button"
            className={
              tone === "primary" ? "luna-btn luna-btn--primary" : "luna-btn luna-btn--secondary"
            }
            onClick={onTry}
          >
            {tryLabel}
          </button>
        ) : (
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onPrepare}>
            {t("step.first_task.prepare")}
          </button>
        )}
      </div>
    </div>
  );
}

export function FirstTaskStep({
  setup,
  onRefresh,
  onGoToStep,
}: {
  setup: SetupStatusDto | null;
  /** 让壳层重读 `setup_status`（权限刚给、任务刚成功）。 */
  onRefresh: () => void;
  /** 能力没就绪时跳回「模型准备」。 */
  onGoToStep: (step: OnboardingStepId) => void;
}) {
  const { t } = useTranslation("onboarding");
  const { specs } = useSettings();
  const [error, setError] = useState<string | null>(null);
  const [micHint, setMicHint] = useState(false);
  // 随包示例在不在是个全程不变的事实，挂载时问一次即可（不进 `setup_status` 那条热路径）。
  const [sampleAvailable, setSampleAvailable] = useState(false);
  const [sampleQueued, setSampleQueued] = useState(false);

  useEffect(() => {
    void commands.onboardingSampleAvailable().then(setSampleAvailable);
  }, []);

  const shortcutValue = (key: string) => {
    const spec = specs.find((s) => s.key === key);
    return spec?.current_value.kind === "str" ? spec.current_value.value : "";
  };
  const voiceShortcut = formatShortcut(shortcutValue("shortcuts.voice_input"));
  // 弹窗翻译触发手势（真实绑定，PRD-09 §2.6 双击键；formatShortcut 已含 ×2 后缀）。
  const translateShortcut = formatShortcut(shortcutValue("shortcuts.popup_translate"));

  const done = setup?.first_task_done === true;
  // 引擎就绪判据取 Rust 下发的那一份（`setup_status`），不从模型目录反推。
  // 状态还没到时按可用渲染：一进门先灰一片比偶尔撞一次权威拒绝更坏（同概览快速开始）。
  //
  // 这一步问的是 `asr_usable`（此刻能不能转写）而**不是** `asr_ready`（推荐档备齐没有）。
  // 两者自随包档落地起就分家了：高配机器的推荐档是 Large v3 Turbo，没装即 `asr_ready=false`，
  // 而随包 tiny 一直在、转写一直跑得起来。用后者判的话，模型页刚说完「内置模型已就绪，
  // 零下载即可开始转写」，这一屏就告诉你「转写模型还没准备好」（真机 2026-09-11）。
  const voiceReady = setup?.asr_usable !== false;
  // 问「翻译与实时档」而不是「有没有对话模型」：只装了专用翻译模型时也能翻（D3）。
  const translateReady = setup?.translate_ready !== false;
  // 示例转写与语音输入用的是同一个本机转写档，差别只在"声音从哪来"。
  const sampleReady = voiceReady && sampleAvailable;

  async function tryVoice() {
    setError(null);
    setMicHint(false);
    // 麦克风按需申请：未询问 → 让系统弹框；被拒 → 说清楚并指去授权，不去撞一个必失败的会话。
    const mic = setup?.microphone ?? (await commands.checkPermission("microphone")).status;
    if (mic === "not_determined") {
      await commands.requestPermission("microphone");
      onRefresh();
    } else if (mic === "denied") {
      setMicHint(true);
      return;
    }
    // 首启引导的"试一次"是普通入口：按用户的持久默认落点（TDD-07 §2.3）。
    const result = await commands.voiceInputShowHud("current");
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
    }
  }

  async function trySample() {
    setError(null);
    setSampleQueued(false);
    const result = await commands.onboardingTranscribeSample();
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      return;
    }
    setSampleQueued(true);
  }

  async function tryTranslate() {
    setError(null);
    // 预填一句示例原文（PRD-14-050）：一个空输入框离"成功时刻"太远。
    const result = await commands.popupShowInput(t("step.first_task.sample_text"));
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
    }
  }

  if (done) {
    // 「还没做完的」名单只认 Rust 下发的 `tracked_ids`——这里此前自己又写了一份排除规则
    // （welcome / compute），于是新加的「快捷键」一步（到访即完成、没有可补的对象）会被
    // 它当成待办列出来。两处各写一份"谁该计入"，迟早对同一件事给出两个分母。
    const trackedIds = new Set(setup?.tracked_ids ?? []);
    const remaining = (setup?.steps ?? []).filter(
      (step) => trackedIds.has(step.id) && step.state !== "done",
    );
    return (
      <div className="luna-onboarding-success" role="status">
        <div className="luna-onboarding-success__badge" aria-hidden="true">
          <CheckIcon size={22} />
        </div>
        <h2 className="luna-onboarding-success__title">{t("step.first_task.success_title")}</h2>
        <p className="luna-onboarding-success__body">{t("step.first_task.success_body")}</p>

        {/* 算力落点是一句陈述，不是一张清单：只剩它一项，再套着列表壳就是一个孤零零的卡。 */}
        <p className="luna-onboarding-success__compute">
          {t("step.first_task.summary_compute")}
          <strong>{t(`step.first_task.location.${setup?.compute_location ?? "on_device"}`)}</strong>
        </p>

        {remaining.length > 0 ? (
          <p className="luna-onboarding__hint">
            {t("step.first_task.remaining_hint", {
              items: remaining.map((step) => t(`step.${step.id}.title`)).join("、"),
            })}
          </p>
        ) : (
          <p className="luna-onboarding__hint">{t("step.first_task.all_done_hint")}</p>
        )}
      </div>
    );
  }

  return (
    <div className="luna-onboarding-step">
      <div className="luna-onboarding-list">
        <TaskCard
        icon={<MicIcon size={18} />}
        title={t("step.first_task.voice_title")}
        desc={
          voiceShortcut
            ? t("step.first_task.voice_desc", { shortcut: voiceShortcut })
            : t("step.first_task.voice_desc_unbound")
        }
        ready={voiceReady}
        notReady={t("step.first_task.voice_not_ready")}
        tryLabel={t("step.first_task.voice_try")}
        tone="primary"
        onTry={() => void tryVoice()}
        onPrepare={() => onGoToStep("models")}
      >
        {micHint ? (
          <p className="luna-onboarding__error" role="alert">
            {t("step.first_task.mic_denied")}{" "}
            <button
              type="button"
              className="luna-onboarding__inline-action"
              onClick={() => {
                void commands.requestPermission("microphone").then(onRefresh);
              }}
            >
              {t("step.permissions.grant")}
            </button>
          </p>
        ) : null}
      </TaskCard>

      <TaskCard
        icon={<LanguagesIcon size={18} />}
        title={t("step.first_task.translate_title")}
        desc={
          translateShortcut
            ? t("step.first_task.translate_desc", { shortcut: translateShortcut })
            : t("step.first_task.translate_desc_unbound")
        }
        ready={translateReady}
        notReady={t("step.first_task.translate_not_ready")}
        tryLabel={t("step.first_task.translate_try")}
        tone="secondary"
        onTry={() => void tryTranslate()}
        onPrepare={() => onGoToStep("models")}
      />

      {sampleAvailable ? (
        <TaskCard
          icon={<MusicIcon size={18} />}
          title={t("step.first_task.clip_title")}
          desc={t("step.first_task.clip_desc")}
          ready={sampleReady}
          notReady={t("step.first_task.clip_not_ready")}
          tryLabel={t("step.first_task.clip_try")}
          tone="secondary"
          onTry={() => void trySample()}
          onPrepare={() => onGoToStep("models")}
        >
          {sampleQueued ? (
            <p className="luna-onboarding__ready" role="status">
              {t("step.first_task.clip_queued")}
            </p>
          ) : null}
          {/* CC BY 4.0 的署名要求是**随作品分发**；包里那份 ATTRIBUTION.md 用户永远看不到，
              所以这一行必须在界面上（完整来源与改动说明仍在那份文件里）。 */}
          <p className="luna-onboarding__hint">{t("step.first_task.clip_credit")}</p>
        </TaskCard>
        ) : null}
      </div>

      <p className="luna-onboarding__hint">
        {voiceReady || translateReady || sampleReady
          ? t("step.first_task.panel_hint")
          : t("step.first_task.none_ready_hint")}
      </p>

      {error ? (
        <p className="luna-onboarding__error" role="alert">
          {t("step.first_task.error")}: {error}
        </p>
      ) : null}
    </div>
  );
}
