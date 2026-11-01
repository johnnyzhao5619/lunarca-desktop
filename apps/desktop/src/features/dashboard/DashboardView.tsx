/**
 * 启动概览（PRD-07-050~054 / PRD-07-057b，macOS 系统设置概览 / Linear Home 对标）。
 *
 * 一屏自上而下回答四个问题，顺序即优先级：
 * 1. 现在系统在干什么 —— 脉搏条（处理队列 + 算力路径），一行看完、可点进去；
 * 2. 此刻最该做什么 —— 「下一步」栈（装机 / 知识库脉搏 / 库体检）。三条来源同屏，
 *    但**只有栈顶那条拿实色主按钮**（裁决在 `next-actions.ts`）：它们各自内部都遵守
 *    「只渲染栈顶一态」，彼此之间却没有仲裁，于是一屏并排三个实色号召，加上继续与
 *    快速开始就是五个——「下一步是什么」这件事反而没人回答；
 * 3. 我上次在干什么 / 现在能干什么 —— 继续 + 快速开始 + 近期笔记（主列，常态入口，
 *    刻意不用实色：它们不是"此刻的下一步"）；
 * 4. 我随手要查的 / 系统明细 —— 侧列：快捷键 → 引擎 → 成效看板。快捷键排第一，
 *    因为它是每天要瞟好几次的速查表，而成效统计看一次就够。
 *
 * 概览是无侧栏、主区全宽的一次性覆层，故用「主列 + 侧列」两栏铺开，而不是把区块纵向
 * 排成一根长条——后者在宽屏上既浪费横向空间，又要滚动两屏才看得到引擎状态与快捷键。
 *
 * 配色纪律（TDD-11 §2.1）：本视图不使用 AI 紫。紫是「这是 AI 在工作 / AI 产物」的语义
 * 标记，刷在录音、新建笔记这类非 AI 入口上等于把语义色降格成装饰色。
 */

import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { KnowledgePulse } from "./KnowledgePulse";
import { KnowledgeCheckupLine } from "../knowledge/KnowledgeCheckupLine";
import { SetupBanner } from "../onboarding/SetupBanner";
import { projectSetupBanner, useSetupStatusContext } from "../onboarding/use-setup-status";
import { nextActionTones } from "./next-actions";
import { isActionable, useKnowledgePulse } from "./use-knowledge-pulse";
import { useCheckupCount } from "../knowledge/use-checkup-count";
import { formatShortcut, HEADLINE_SHORTCUT_KEYS } from "../../lib/shortcut-display";
import { useModels, useSettings } from "../settings/use-settings";
import { useCreateActionStates } from "../capture/AddContentPopover";
import { recordCreateEntry } from "../workspace/use-workspace-metrics";
import {
  CREATE_ACTIONS,
  CREATE_ACTION_REASON_KEYS,
  CREATE_ACTION_REPAIR_LABEL_KEYS,
  runCreateActionRepair,
  type CreateActionContext,
} from "../capture/create-actions";
import { useUsage } from "../usage/use-usage";
import {
  commands,
  events,
  type ComputeStatusDto,
  type CreateActionIdDto,
  type SearchHitDto,
} from "../../ipc/bindings";
import { activeCount, useProcessingQueueSelector } from "../processing/use-processing-queue";
import { formatCount } from "../../lib/format-datetime";
import { formatDuration } from "../../lib/format-duration";
import { formatUsd } from "../../lib/format-money";
import { formatRelativeTime } from "../../lib/format-relative-time";
import { TaskStatus } from "../../components/TaskStatus";
import { goTo, openNote } from "../../lib/navigate";
import {
  FileIcon,
  ShieldIcon,
} from "../../components/ui/Icon";
import {
  projectDashboardEngines,
  type DashboardEngineItem,
} from "./dashboard-engine-status";

/**
 * 概览不自持动作表（PRD-07-068 / TDD-07 §4.2c）：「快速开始」= 用户配置的 id 子集 ×
 * 唯一注册表 `features/capture/create-actions`，与「添加内容」消费同一份可用性快照。
 * 各写一份的话，"有哪些添加方式"在应用里就有两个答案。
 */

/** 近期记录展示条数（含被「继续上一次工作」提走的那条，故取数多要一条）。 */
const RECENT_LIMIT = 5;

export interface DashboardViewProps {
  /** 宿主（启动概览弹窗）的关闭回调：脉搏区块的跨视图动作先关弹窗再行动。 */
  onDismiss?: () => void;
  /** 五类创建动作的执行上下文——与「添加内容」注入的是**同一个**（PRD-07-068）。 */
  createActions: CreateActionContext;
}

export function DashboardView({ onDismiss, createActions }: DashboardViewProps) {
  const { t, i18n } = useTranslation("workspace");
  const { summary } = useUsage();
  const { specs } = useSettings();
  // App 层唯一的装机订阅（与横幅、引导同一份）——概览不另读一遍。
  const { setup } = useSetupStatusContext();
  const { snapshot: actionStates } = useCreateActionStates();
  /**
   * 「快速开始」= 用户配置的 id 子集（PRD-07-068 / TDD-05 §7.5a）。
   *
   * 不在这里兜底成"默认四项"：默认值本身就是设置的默认值，Rust 已经给过了；前端再写
   * 一份，两处默认迟早不一样。读不到就是空——那只发生在设置尚未加载的一瞬。
   */
  const quickStart = useMemo(() => {
    const value = specs.find((spec) => spec.key === "workspace.quick_start_actions")?.current_value;
    const ids = value?.kind === "string_list" ? value.value : [];
    // 未知 id 一律丢弃：设置里可能留着一个这个版本已经不认识的动作，而渲染它只会崩。
    return ids.filter((id): id is CreateActionIdDto => id in CREATE_ACTIONS);
  }, [specs]);
  const { models } = useModels();
  const [recentNotes, setRecentNotes] = useState<SearchHitDto[]>([]);
  /** 近期记录自身的加载态——此前借用了用量摘要的 `loading`，两者根本不是同一次请求。 */
  const [recentLoading, setRecentLoading] = useState(true);
  const [computeStatus, setComputeStatus] = useState<ComputeStatusDto | null>(null);
  const [computeResolved, setComputeResolved] = useState(false);
  /**
   * 相对时间的「现在」。概览不是秒表，不逐秒重算；但它可以被开着很久（用户去干别的、
   * 回来接着看），一次性快照会让「3 分钟前」在半小时后还写着 3 分钟前。每分钟走一格
   * 足够诚实，也远低于任何可感知的开销。
   */
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    void commands
      .search({ text: null, note_type: null, tag: null, path_prefix: null, limit: RECENT_LIMIT })
      .then((res) => {
        if (res.status === "ok") {
          setRecentNotes(res.data);
        }
        setRecentLoading(false);
      });
  }, []);

  useEffect(() => {
    const refresh = async () => {
      const result = await commands.computeStatus();
      setComputeStatus(result.status === "ok" ? result.data : null);
      setComputeResolved(true);
    };
    void refresh();
    const unlisten = events.settingsChanged.listen(() => void refresh());
    return () => void unlisten.then((dispose) => dispose());
  }, []);

  // 快捷键展示直接派生自真实绑定（settings registry 单一来源 + shortcut-display 展示规约）。
  const shortcutValue = (key: string) => {
    const value = specs.find((s) => s.key === key)?.current_value;
    return value?.kind === "str" ? value.value : "";
  };
  // 哪三个键、什么顺序取共用的 `HEADLINE_SHORTCUT_KEYS`（引导的「快捷键」一步列的是
  // 同一组）——此前这里手写了一遍同样的三个 key，两处要改都得有人同时记得。
  // 速查文案仍归本页自己：引导那边列的是可编辑的设置项标签，这里是只读速查。
  const SHORTCUT_LABEL_KEYS: Record<(typeof HEADLINE_SHORTCUT_KEYS)[number], string> = {
    "shortcuts.popup_translate": "dashboard.shortcuts.translate",
    "shortcuts.voice_input": "dashboard.shortcuts.voice",
    "shortcuts.quick_chat": "dashboard.shortcuts.chat",
  };
  const shortcuts = HEADLINE_SHORTCUT_KEYS.map((key) => ({
    labelKey: SHORTCUT_LABEL_KEYS[key],
    value: formatShortcut(shortcutValue(key)),
  }));

  const asrSecs = summary?.audio_seconds ?? 0;
  // 千分位与本地化走与「设置 › 用量」同一个 `formatCount`：同一个数在两处两种长相，
  // 是用户第一眼就会怀疑"这两处说的是不是一回事"的那类不一致。
  const tokenIn = Number(summary?.tokens_in ?? 0);
  const tokenOut = Number(summary?.tokens_out ?? 0);
  // 「本地节省」估算在 Rust 侧算好下发，前端只渲染（ADR-003 前端零业务）。
  // `null` = 用户没配参考单价 → 不展示金额，改给一条去配的路（PRD-07-051）。
  const savings = summary?.estimated_savings_usd ?? null;

  // 执行路径只认 Rust `compute_status`；本机目录只在 Rust 已裁决为 device 时补模型展示名。
  // 服务端托管形态允许本机零模型，不能再由 `list_models` 反推为「引擎未就绪」。
  const engineProjection = useMemo(
    () => projectDashboardEngines(computeStatus, models, computeResolved),
    [computeStatus, models, computeResolved],
  );

  const engineDetail = (item: DashboardEngineItem) => {
    if (item.source === "managed_service") return t("dashboard.engine.managedService");
    if (item.source === "device") return item.modelName ?? t("dashboard.engine.noModel");
    return t("dashboard.engine.statusUnavailable");
  };

  const engineStatusLabel = (item: DashboardEngineItem) => {
    if (item.source === "managed_service") return t("dashboard.engine.server");
    if (item.source === "unavailable") return t("dashboard.engine.statusUnavailable");
    return item.ready ? t("dashboard.engine.device") : t("dashboard.engine.needsSetup");
  };

  // 与顶栏队列入口**同一份**模块级订阅，只取在跑的条数（进度推进不重渲染概览）。
  const queueActive = useProcessingQueueSelector((state) => activeCount(state.items));

  // 「下一步」三条来源的语气仲裁：同屏至多一条实色主按钮。三条的取数都提到这一层，
  // 因为仲裁需要知道每条此刻有没有话说——那件事此前只有各行自己知道。
  const knowledgePulse = useKnowledgePulse();
  const checkup = useCheckupCount();
  const setupPending =
    setup !== null && setup.completed && projectSetupBanner(setup).firstRemaining !== null;
  const tones = nextActionTones({
    setup: setupPending,
    knowledge: isActionable(knowledgePulse.state),
  });
  // 整区等三条都到齐再画：它们各自到达、各自决定要不要渲染的话，弹窗打开后高度会连跳
  // 两三次，而用户正在读的就是这几行。装机那份来自 App 层已有的订阅（`setup === null`
  // 只在最初一瞬）；另外两条各自带 `loaded`。
  const nextActionsReady = setup !== null && knowledgePulse.loaded && checkup.loaded;

  // 「继续」取最近一条；近期列表**去掉它**——同一条在一屏里出现两次是纯噪声。
  const continueItem = recentNotes[0] ?? null;
  const restNotes = recentNotes.slice(1);

  return (
    <div className="luna-dashboard">
      {/* 首屏第一件东西就是状态条（PRD-07-065 同一事实同屏只说一次）：宿主的对话框页头
          已经写了「概览」，这里此前还叠着品牌眉标签、一句标题和一句产品说明——四行标题类
          文本占掉首屏一百多像素，而它们没有一行在回答「现在该干什么」。品牌名对已经在用
          这个产品的人零信息；那句「采集、处理、复核、沉淀」是产品说明书，位置在引导欢迎页。
          标题拿走之后 `<header>` 里只剩这一条，空的 landmark 反倒是读屏上的噪声，故一并去掉。 */}
      <div className="luna-dashboard__pulse" role="group" aria-label={t("dashboard.pulse.label")}>
        <button
          type="button"
          className="luna-dashboard__pulse-item"
          data-tone={queueActive > 0 ? "busy" : "idle"}
          // 「现在什么在处理」→ 顶栏处理队列面板（PRD-07-056）。此前它指向「动态」
          // 聚合页，而那个容器已随 ADR-087 ② 退役、旧路由被明确拒绝。
          onClick={() => goTo("queue://")}
        >
          <span className="luna-dashboard__pulse-dot" aria-hidden="true" />
          {queueActive > 0
            ? t("dashboard.pulse.queueBusy", { count: queueActive })
            : t("dashboard.pulse.queueIdle")}
        </button>
        <button
          type="button"
          className="luna-dashboard__pulse-item"
          data-tone={
            engineProjection.pulse === "needs_setup" || engineProjection.pulse === "unavailable"
              ? "attention"
              : "idle"
          }
          onClick={() => goTo("settings://ai_services")}
        >
          <span className="luna-dashboard__pulse-dot" aria-hidden="true" />
          {t(`dashboard.pulse.compute.${engineProjection.pulse}`)}
        </button>
      </div>

      {/* 「下一步」栈：三条来源同屏，但只有栈顶那条拿实色主按钮（裁决在 `next-actions.ts`）。
          装机排最前——「有事才弹」的两个触发条件之一就是它有内容，而概览此前只字不提装机，
          用户被打断了却拿不到被打断的理由，关掉概览才在底下看见横幅。 */}
      {nextActionsReady ? (
        <>
          <SetupBanner setup={setup} variant="row" onDismiss={onDismiss} />
          <KnowledgePulse pulse={knowledgePulse} tone={tones.knowledge} onDismiss={onDismiss} />
          <KnowledgeCheckupLine
            count={checkup.count}
            tone={tones.checkup}
            onDismiss={onDismiss}
          />
        </>
      ) : null}
      <div className="luna-dashboard__columns">
        <div className="luna-dashboard__column">
          {/* 继续上一次工作：回到上次的上下文。它是常态入口，不是"此刻的下一步"——
              后者归上面那个栈，那里已经有一条实色主按钮了。 */}
          <section className="luna-dashboard-continue" aria-labelledby="dashboard-continue-title">
            <div className="luna-dashboard-continue__heading">
              <h2 id="dashboard-continue-title">{t("dashboard.continue.title")}</h2>
              {/* 有笔记时不挂徽标：最近编辑过的一篇不是一件待办，标成「需要行动」
                  是在给用户制造虚假负债。只有空态需要说一句"还没有"。 */}
              {continueItem ? null : (
                <TaskStatus kind="idle" label={t("dashboard.continue.emptyStatus")} />
              )}
            </div>
            {continueItem ? (
              <button
                type="button"
                className="luna-dashboard-continue__item"
                onClick={() => openNote(continueItem.path)}
              >
                <FileIcon size={16} />
                <span>
                  <strong>{continueItem.title}</strong>
                  <small>{t("dashboard.continue.openHint", { path: continueItem.path })}</small>
                </span>
              </button>
            ) : (
              <p className="luna-dashboard-continue__empty">{t("dashboard.continue.empty")}</p>
            )}
          </section>

          {/* 快速开始行动卡片（PRD-00 §6：首屏应引导「你可以做什么」） */}
          <section className="luna-dashboard-actions">
            <div className="luna-dashboard-actions__heading">
              <h2 className="luna-dashboard-actions__title">{t("dashboard.actions.title")}</h2>
              <p>{t("dashboard.actions.description")}</p>
            </div>
            <div className="luna-dashboard-actions__grid">
              {quickStart.map((id) => {
                const spec = CREATE_ACTIONS[id];
                const entry = actionStates?.actions.find((it) => it.id === id);
                // 状态还没到就当可用：一进门先灰一片，比偶尔点一次拿到 Rust 的权威拒绝
                // 更糟——后者至少说得清为什么（执行命令自己会再校验一次，快照不是凭证）。
                const blocked = entry !== undefined && entry.state.kind !== "available";
                const reason = entry && entry.state.kind !== "available" ? entry.state.reason : null;
                const repair =
                  entry && entry.state.kind === "needs_attention" ? entry.state.repair : null;
                return (
                  // 卡片与"为什么不能用 / 怎么解决"是**兄弟**而不是父子：禁用的按钮不可
                  // 聚焦，把原因塞进它的 `title` 等于键盘与读屏用户根本拿不到，而修复钮
                  // 更不可能嵌在一个禁用按钮里（PRD-07-068 要求两者都保留）。
                  <div key={id} className="luna-dashboard-action-cell">
                    <button
                      type="button"
                      className="luna-dashboard-action-card"
                      disabled={blocked}
                      onClick={() => {
                        const startedAt = performance.now();
                        onDismiss?.();
                        // 入口面是「快速开始」——它与「添加内容」是同一动作的两个入口，
                        // 分开记才答得了"用户更常从哪儿开始"（PRD-07-070）。终态由动作
                        // 自己回报，不一律记成到达。
                        void spec
                          .run(createActions)
                          .then((result) => recordCreateEntry(id, "quick_start", result, startedAt));
                      }}
                    >
                      <span className="luna-dashboard-action-card__icon" aria-hidden="true">
                        {spec.icon}
                      </span>
                      <span>{t(spec.labelKey)}</span>
                      {/* 产物去向与「添加内容」同源同文案（PRD-07-069）：概览这一屏更
                          需要它——新用户正是在这里第一次面对"这两个麦克风有什么区别"。 */}
                      <span className="luna-dashboard-action-card__destination">
                        {t(spec.destinationKey)}
                      </span>
                    </button>
                    {reason && (
                      <p className="luna-dashboard-action-cell__reason">
                        {t(CREATE_ACTION_REASON_KEYS[reason])}
                        {repair && (
                          <button
                            type="button"
                            className="luna-dashboard-action-cell__repair"
                            onClick={() => {
                              // 点「去解决」本身就是一条 `rejected`，口径与「添加内容」
                              // 那个入口**逐字一致**（`AddContentPopover.runRepair`）：同一个
                              // 动作在两个面上记法不同，PRD-07-070 的入口到达率两面就不可比
                              // ——而"用户更常从哪儿开始"正是这条指标要回答的问题。
                              recordCreateEntry(id, "quick_start", "rejected", performance.now());
                              onDismiss?.();
                              runCreateActionRepair(repair, createActions);
                            }}
                          >
                            {t(CREATE_ACTION_REPAIR_LABEL_KEYS[repair.kind])}
                          </button>
                        )}
                      </p>
                    )}
                  </div>
                );
              })}
            </div>
          </section>

          {/* 近期记录（只列笔记；通知历史归状态栏铃铛的应用级通知抽屉，ADR-087 ②） */}
          <section className="luna-dashboard-section">
            <div className="luna-dashboard-section__heading">
              <h2>{t("dashboard.history.title")}</h2>
            </div>
            {recentLoading ? (
              <p className="luna-empty">{t("common:status.loading")}</p>
            ) : restNotes.length === 0 ? (
              <p className="luna-empty">{t("dashboard.history.empty")}</p>
            ) : (
              <ul className="luna-dashboard-history-list">
                {restNotes.map((note) => (
                  <li key={note.path}>
                    <button
                      type="button"
                      className="luna-dashboard-history-item"
                      onClick={() => openNote(note.path)}
                    >
                      <span className="luna-dashboard-history-item__left">
                        <strong>{note.title}</strong>
                        <span>{note.path}</span>
                      </span>
                      <time className="luna-dashboard-history-item__time">
                        {formatRelativeTime(note.modified_ms, now, i18n.language)}
                      </time>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </div>

        <aside className="luna-dashboard__column">
          {/* 快捷键 Cheatsheet（PRD-07-053）：值派生自真实绑定，改键直达设置 */}
          <section className="luna-dashboard-section">
            <div className="luna-dashboard-section__heading">
              <h2>{t("dashboard.shortcuts.title")}</h2>
            </div>
            <div className="luna-shortcuts-list">
              {shortcuts.map(({ labelKey, value }) => (
                <div key={labelKey} className="luna-shortcut-item">
                  <span>{t(labelKey)}</span>
                  <kbd>{value || t("dashboard.shortcuts.unbound")}</kbd>
                </div>
              ))}
            </div>
            <button
              type="button"
              className="luna-dashboard-section__link"
              onClick={() => goTo("settings://shortcuts")}
            >
              {t("dashboard.shortcuts.manage")}
            </button>
          </section>

          {/* AI 算力与本地数据（PRD-07-052）：真实路径来自 compute_status。 */}
          <section className="luna-dashboard-section">
            <div className="luna-dashboard-section__heading">
              <h2>{t("dashboard.engine.title")}</h2>
              <ShieldIcon size={18} aria-hidden="true" />
            </div>
            <div className="luna-engine-status-list">
              <div className="luna-engine-status-item">
                <div className="luna-engine-status-item__left">
                  <strong>{t("dashboard.engine.asr")}</strong>
                  <span>{engineDetail(engineProjection.asr)}</span>
                </div>
                <TaskStatus
                  kind={engineProjection.asr.ready ? "completed" : "needs_action"}
                  label={engineStatusLabel(engineProjection.asr)}
                />
              </div>
              <div className="luna-engine-status-item">
                <div className="luna-engine-status-item__left">
                  <strong>{t("dashboard.engine.llm")}</strong>
                  <span>{engineDetail(engineProjection.llm)}</span>
                </div>
                <TaskStatus
                  kind={engineProjection.llm.ready ? "completed" : "needs_action"}
                  label={engineStatusLabel(engineProjection.llm)}
                />
              </div>
              {/* 这里此前还有第三行「数据隐私保护 · 恒为已完成」。它不是状态——它是一句
                  恒真的品牌承诺，混在两条真实路由中间，既没有下一步（PRD-07-057b 要求
                  每张卡有下一步），又让另外两行的「已就绪 / 需设置」看起来像同一类东西。
                  这句话的位置在引导欢迎页的隐私承诺与设置的数据/隐私组，两处都已经在说。 */}
            </div>
            <button
              type="button"
              className="luna-dashboard-section__link"
              onClick={() => goTo("settings://ai_services")}
            >
              {t("dashboard.engine.manage")}
            </button>
          </section>

          {/* 成效看板（PRD-07-051）：节省额高亮，其余三项等权 */}
          <section className="luna-dashboard-section">
            <div className="luna-dashboard-section__heading">
              <h2>{t("dashboard.metrics.title")}</h2>
            </div>
            <div className="luna-dashboard-metrics">
              {/* 没配参考单价就不给金额（PRD-07-051）：此前拿注册表里预置的厂商报价
                  算出一个数，用户既没设过、也看不到它从哪来。 */}
              <div className="luna-dashboard-metric luna-dashboard-metric--highlight">
                <span className="luna-dashboard-metric__label">
                  {t("dashboard.metrics.savings")}
                </span>
                {savings === null ? (
                  <button
                    type="button"
                    className="luna-dashboard-section__link"
                    onClick={() => goTo("settings://ai_services/provider.asr_cost_per_min")}
                  >
                    {t("dashboard.metrics.savingsSetPrice")}
                  </button>
                ) : (
                  <>
                    <strong className="luna-dashboard-metric__value">
                      {formatUsd(savings, i18n.language)}
                    </strong>
                    <span className="luna-dashboard-metric__sub">
                      {t("dashboard.metrics.localComputeDesc")}
                    </span>
                  </>
                )}
              </div>
              <div className="luna-dashboard-metric">
                <span className="luna-dashboard-metric__label">
                  {t("dashboard.metrics.totalJobs")}
                </span>
                <strong className="luna-dashboard-metric__value">
                  {formatCount(summary?.records, i18n.language)}
                </strong>
              </div>
              <div className="luna-dashboard-metric">
                <span className="luna-dashboard-metric__label">
                  {t("dashboard.metrics.dictationTime")}
                </span>
                <strong className="luna-dashboard-metric__value">{formatDuration(asrSecs)}</strong>
              </div>
              <div className="luna-dashboard-metric">
                <span className="luna-dashboard-metric__label">
                  {t("dashboard.metrics.tokens")}
                </span>
                <strong className="luna-dashboard-metric__value">
                  {formatCount(tokenIn + tokenOut, i18n.language)}
                </strong>
              </div>
            </div>
            {/* 「每张卡有下一步」（PRD-07-057b）：摘要之后是逐条执行记录，落点是
                **唯一**的用量深链 `settings://ai_services/usage`（PRD-07-056）。 */}
            <button
              type="button"
              className="luna-dashboard-section__link"
              onClick={() => goTo("settings://ai_services/usage")}
            >
              {t("dashboard.metrics.viewRecords")}
            </button>
          </section>
        </aside>
      </div>
    </div>
  );
}
