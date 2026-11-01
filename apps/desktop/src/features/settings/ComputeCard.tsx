/**
 * 「AI 在哪跑」状态卡片 —— AI 算力与模型页的首屏。
 *
 * 状态驱动而不是三选一：只呈现此刻是什么状态 + 这个状态下唯一有意义的下一步。
 * 状态由 Rust 一次算清（`compute_status`，ADR-003）；前端不拿隐私档 + 登录态 + 地址
 * 自己拼——那是同一件事在不同界面上说法不同的来源。
 */

import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDownIcon } from "../../components/ui/Icon";
import { goTo } from "../../lib/navigate";
import { useAccount } from "../../hooks/use-account";
import { refreshComputeStatus, useComputeStatus } from "../../hooks/use-compute-status";
import { PanelSettingRow } from "./PanelSettingRow";
import { ComputeSelfCheck } from "./ComputeSelfCheck";
import { TranscriptionConsentRow, TRANSCRIPTION_CONSENT_KEY } from "./TranscriptionConsentRow";
import {
  commands,
  type ComputeLocation,
  type ComputeRouteDto,
  type ComputeStatusDto,
  type LocalCandidateDto,
  type LocalCandidatePolicy,
  type LocalCandidateReadiness,
  type SettingSpecDto,
  type SettingValueDto,
} from "../../ipc/bindings";

/** 优先落点：它决定的正是本卡上逐条能力的去向（ADR-122 第 1 条）。 */
const ROUTE_STRATEGY_KEY = "ai.route_strategy";
/** 登录、退出、换服务器的唯一入口（ADR-155）。 */
const ACCOUNT_ROUTE = "settings://account";
/** 本机候选缺模型 / 引擎起不来时的去处：本地模型页的模型库。 */
const LOCAL_MODELS_ROUTE = "settings://local_models/models";
/** 严格本地下唯一有意义的动作是回到隐私档本身。 */
const STRICT_LOCAL_ROUTE = "settings://privacy/privacy.strict_local";

/** 每种落点的标题与说明。穷尽 Record 而不是模板拼 key：新增一种落点时 tsc 立刻红。 */
export const LOCATION_COPY: Record<ComputeLocation, { name: string; body: string }> = {
  strict_local: { name: "compute_card.strict_local", body: "compute_card.strict_local_body" },
  on_device: { name: "compute_card.on_device", body: "compute_card.on_device_body" },
  official_cloud: { name: "compute_card.official_cloud", body: "compute_card.official_cloud_body" },
  organization: { name: "compute_card.organization", body: "compute_card.organization_body" },
};

/** 连着服务端、但「优先在哪运行」设成了仅本机：卡头说的必须是这件事，而不是「请求发往云」。 */
export const LOCAL_ONLY_COPY = {
  name: "compute_card.local_only",
  body: "compute_card.local_only_body",
};

/** 连着的服务端一个上游都没登记（ADR-041 ①）：说「它尚未配置算力」，不说「请求发往服务端」。 */
export const SETUP_INCOMPLETE_COPY = {
  name: "compute_card.setup_incomplete",
  body: "compute_card.setup_incomplete_body",
};

/**
 * 卡头说哪一句。服务端没配算力在前：那时连「仅本机」也无从谈起——请求本来就到不了它。
 */
export function headCopy(status: ComputeStatusDto): { name: string; body: string } {
  if (!runsOnServer(status.location)) return LOCATION_COPY[status.location];
  if (status.server_setup_incomplete) return SETUP_INCOMPLETE_COPY;
  if (status.outbound_blocked_by_strategy) return LOCAL_ONLY_COPY;
  return LOCATION_COPY[status.location];
}

const LOCAL_CANDIDATE_POLICY_COPY: Record<LocalCandidatePolicy, string> = {
  unreachable_before_visible_output: "compute_card.local_candidate.policy.unreachable_before_visible_output",
  explicit_only: "compute_card.local_candidate.policy.explicit_only",
};

const LOCAL_CANDIDATE_READINESS_COPY: Record<LocalCandidateReadiness, string> = {
  ready: "compute_card.local_candidate.readiness.ready",
  model_missing: "compute_card.local_candidate.readiness.model_missing",
  runtime_unavailable: "compute_card.local_candidate.readiness.runtime_unavailable",
  not_applicable: "compute_card.local_candidate.readiness.not_applicable",
};

/** 算力落在服务端（官方云或组织自建）——两者对用户的差别只是「谁的服务器」。 */
export function runsOnServer(location: ComputeLocation): boolean {
  return location === "official_cloud" || location === "organization";
}

/**
 * 「在服务端跑」这一格写哪个名字——**与卡头同屏就必须是同一个词**（ADR-126 第 14 条）。
 */
export function serverLabelKey(status: ComputeStatusDto): string {
  return status.location === "organization"
    ? "compute_card.route.server_organization"
    : "compute_card.route.server_cloud";
}

/**
 * 「现在到底怎么跑的」——**两行，位置在前**（ADR-127）。
 *
 * 标准投影下逐项那张表收进折叠，而**隐私事实不能跟着收起来**：Apple Intelligence 的
 * 设置里只有一个开关、执行位置要去另一个默认关闭的报告里看，那一点恰恰是它被批评的
 * 地方（用户无法在当下判断这次请求出没出设备）。这两行就是堵住那个口子的东西。
 *
 * 位置排在能力前面：用户带来的问题是「哪些出了这台设备」，而不是「对话在哪」——
 * 位置在行首时两行的起点对齐，一眼就能分出两组。
 * 能力名之间的连接交给 `Intl.ListFormat`：中文的顿号与英文的 and 由语言决定，
 * 在文案里手写分隔符会在第三种语言上露馅。
 */
function summaryLines(
  status: ComputeStatusDto,
  t: (key: string, options?: Record<string, unknown>) => string,
  language: string,
): { key: string; text: string }[] {
  // **默认档（long conjunction），不赌 `narrow`**：真机上 WKWebView 与 Node 的 ICU 数据
  // 不一致——同一份 `narrow` 在 Node 给顿号、在 WKWebView 仍给「和」。躲连接词这件事
  // 改由**能力名自带顿号**解决（`inline.chat` = 「对话、翻译」），于是连起来是
  // 「对话、翻译和语义索引」，通顺且与引擎无关。
  const list = new Intl.ListFormat(language, { style: "long", type: "conjunction" });
  // 句中形态：英语的功能名在句中不首字母大写，而逐项表里那份是独立标签。同一个名字的
  // 两种书写形态，不是两个名字。
  const name = (route: ComputeRouteDto) => t(`compute_card.route.inline.${route.capability}`);
  const server = status.routes.filter((route) => route.location === "server").map(name);
  const local = status.routes.filter((route) => route.location !== "server").map(name);
  const lines: { key: string; text: string }[] = [];
  if (server.length > 0) {
    lines.push({
      key: "server",
      text: t("compute_card.summary.server", {
        where: t(
          status.location === "organization"
            ? "compute_card.route.server_organization"
            : "compute_card.route.server_cloud",
        ),
        names: list.format(server),
      }),
    });
  }
  if (local.length > 0) {
    // 一条都不外发时把这件事说出来：那是这一档唯一的卖点，而它在混合态里不成立。
    lines.push({
      key: "local",
      text: t(server.length === 0 ? "compute_card.summary.local_only" : "compute_card.summary.local", {
        names: list.format(local),
      }),
    });
  }
  return lines;
}

/**
 * 落在本机却还用不了的能力，按原因各成一行（缺模型 / 引擎起不来是两种下一步）。
 * 只看**此刻落在本机**的那几条：在服务端跑的能力，本机候选缺模型只影响断线兜底，
 * 那一层已在逐项表的「本机备用」里说过。
 */
function unreadyLines(
  status: ComputeStatusDto,
  t: (key: string, options?: Record<string, unknown>) => string,
  language: string,
): { readiness: "model_missing" | "runtime_unavailable"; text: string }[] {
  const list = new Intl.ListFormat(language, { style: "long", type: "conjunction" });
  const lines: { readiness: "model_missing" | "runtime_unavailable"; text: string }[] = [];
  for (const readiness of ["model_missing", "runtime_unavailable"] as const) {
    const names = status.routes
      .filter((route) => route.location !== "server")
      .filter(
        (route) =>
          status.local_candidates.find((candidate) => candidate.capability === route.capability)
            ?.readiness === readiness,
      )
      .map((route) => t(`compute_card.route.inline.${route.capability}`));
    if (names.length > 0) {
      lines.push({
        readiness,
        text: t(`compute_card.summary.unready_${readiness}`, { names: list.format(names) }),
      });
    }
  }
  return lines;
}

export type ComputeCardProps = {
  /** 复用控制器的写入路径——错误落到行级、广播 `settings://changed`，与设置行同一套。 */
  setValue: (key: string, value: SettingValueDto) => Promise<SettingValueDto | null>;
  /** 取注册表声明：本卡承载的键要渲染注册表声明的控件形态（ADR-122 第 5 条）。 */
  specOf: (key: string) => SettingSpecDto | undefined;
};

/**
 * 一条能力的本机候选，两态共用同一条记录：在服务端跑时它是备用，在本机跑时它就是
 * 正在用的那一个（ADR-126 第 2 条）。
 */
function LocalCandidate({ route, candidate }: { route: ComputeRouteDto; candidate: LocalCandidateDto }) {
  const { t } = useTranslation("settings");
  const active = route.location !== "server";
  // 正在用的那一个只说它是谁；备用的还要说接管边界。
  const showPolicy = !active || candidate.readiness === "not_applicable";
  const showReadiness = candidate.readiness !== "not_applicable";
  return (
    <div
      className="luna-compute-card__candidate"
      data-role={active ? "active" : "fallback"}
      data-readiness={candidate.readiness}
    >
      <span className="luna-compute-card__candidate-label">
        {t(active ? "compute_card.local_candidate.active_title" : "compute_card.local_candidate.title")}
      </span>
      {showPolicy ? <small>{t(LOCAL_CANDIDATE_POLICY_COPY[candidate.policy])}</small> : null}
      {showReadiness ? (
        <small>
          {t(LOCAL_CANDIDATE_READINESS_COPY[candidate.readiness], {
            model: candidate.model_name ?? t("compute_card.local_candidate.no_model"),
          })}
        </small>
      ) : null}
    </div>
  );
}

/** 纯呈现层：只吃一份已算好的状态。拆出来是为了让四种落点都能被静态渲染断言。 */
export function ComputeCardView({
  status,
  setValue,
  specOf,
  resolveHandover,
  showingAdvanced = false,
  serverName = null,
}: ComputeCardProps & {
  status: ComputeStatusDto;
  /** 组织服务器的称呼（账户状态给的：部署名，否则地址）；其余落点不写。 */
  serverName?: string | null;
  /** 交接提示的两个出口（`true` = 交给服务端）。写入路径在 Rust，前端只发命令。 */
  resolveHandover: (handOver: boolean) => Promise<void>;
  /** 完整设置下逐项那张表常驻；标准投影下它收进折叠（ADR-127）。 */
  showingAdvanced?: boolean;
}) {
  const { t, i18n } = useTranslation("settings");
  // 折叠改受控（不用 `details`）：入口要落在卡头右端，而 `details` 的 summary 必须与
  // 内容同父——那会让它独占一行。`aria-expanded` 手给，语义等价（ADR-127 第 7 条）。
  const [routesOpen, setRoutesOpen] = useState(showingAdvanced);
  const onServer = runsOnServer(status.location);
  const strictLocal = status.location === "strict_local";
  const copy = headCopy(status);
  const transcriptionRoute = status.routes.find((route) => route.capability === "transcription");
  const transcriptionCandidate = status.local_candidates.find(
    (candidate) => candidate.capability === "transcription",
  );
  // 未登录：AI 全在本机。给一个去账户页的入口——登录了哪台服务器，AI 就由谁提供。
  const signedOut = status.location === "on_device";

  return (
    <section className="luna-settings-card luna-compute-card" aria-label={t("compute_card.title")}>
      {/* ── 状态带：现在怎样 ────────────────────────────────────────────
          结果先说完再说设定（ADR-127）。此前顺序是「结果 → 设定 → 结果」，
          左边缘还分两条起始线（设置行自带内缩，而说明句是卡片级左对齐）。 */}
      <header className="luna-compute-card__head">
        <span className="luna-compute-card__where" data-location={status.location}>
          {t(copy.name)}
        </span>
        {/* 只有组织形态写是哪一台。官方云没有「哪一台」这个概念——它背后是内置默认值，
            贴出来像一台内网机器（ADR-125 第 11 条）。 */}
        {status.location === "organization" && serverName ? (
          <span className="luna-compute-card__address">{serverName}</span>
        ) : null}
        {/* 完整设置下逐项表常驻，这颗入口就没有存在的理由。 */}
        {showingAdvanced ? null : (
          <button
            type="button"
            className="luna-compute-card__routes-toggle"
            aria-expanded={routesOpen}
            onClick={() => setRoutesOpen((open) => !open)}
          >
            <ChevronDownIcon size={13} aria-hidden />
            <span>{t("compute_card.routes_title")}</span>
          </button>
        )}
      </header>

      <div className="luna-compute-card__summary">
        {summaryLines(status, t, i18n.language).map((line) => (
          <p key={line.key} data-where={line.key}>
            {line.text}
          </p>
        ))}
      </div>

      {/* 「在这台设备上运行」不等于「能用」（2026-10-08 F15/F16）：落在本机、而本机候选缺模型或
          引擎起不来的能力，在折叠表之外常显一行，并就地给去处。判据是 Rust 的 readiness。 */}
      {unreadyLines(status, t, i18n.language).map((line) => (
        <p key={line.readiness} className="luna-compute-card__unready" role="note">
          <span>{line.text}</span>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => goTo(LOCAL_MODELS_ROUTE)}
          >
            {t("compute_card.action.local_models")}
          </button>
        </p>
      ))}

      {/* 逐项表：标准投影下收在卡头那颗入口后面，完整设置下常驻。
          一组能力只列一次（ADR-122 第 3 条）。 */}
      {showingAdvanced || routesOpen ? (
        <dl className="luna-compute-card__routes" aria-label={t("compute_card.routes_title")}>
          {status.routes.map((route) => {
            const candidate = status.local_candidates.find(
              (item) => item.capability === route.capability,
            );
            return (
              <div key={route.capability} className="luna-compute-card__route">
                <dt>{t(`compute_card.route.${route.capability}`)}</dt>
                <dd data-location={route.location === "server" ? "server" : "local"}>
                  <span className="luna-compute-card__route-where">
                    {t(
                      route.location === "server"
                        ? serverLabelKey(status)
                        : "compute_card.route.local",
                    )}
                  </span>
                  <small className="luna-compute-card__route-why">
                    {t(`compute_card.reason.${route.reason}`)}
                  </small>
                  {candidate ? <LocalCandidate route={route} candidate={candidate} /> : null}
                  {/* 三格一律给「更改」，且只有一种去处（ADR-126 第 5 条）。它回答「用哪个
                      模型」——「要不要交给服务端」是另一件事，归设置带那枚同意开关。 */}
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary luna-btn--sm luna-compute-card__route-switch"
                    // 去处写进属性而不只活在闭包里：否则"三格是不是落到同一种地方"验不了。
                    data-route={`settings://ai_services/${route.setting_key}`}
                    onClick={() => goTo(`settings://ai_services/${route.setting_key}`)}
                  >
                    {t("compute_card.switch")}
                  </button>
                </dd>
              </div>
            );
          })}
        </dl>
      ) : null}

      {/* 承诺句：它是背书不是状态，故收在状态带末尾；主语交给卡头，不再重复。 */}
      <p className="luna-compute-card__body">{t(copy.body)}</p>

      {/* ── 设置带：两个可改项连续排列，右侧控件因此对齐成一条线 ──────── */}
      <div className="luna-compute-card__controls">
        <PanelSettingRow
          spec={specOf(ROUTE_STRATEGY_KEY)}
          setValue={setValue}
          disabled={strictLocal ? true : undefined}
          desc={strictLocal ? t("compute_card.strategy_strict_local") : undefined}
        />
        {/* 转写同意（ADR-038 第 4 条）不与「连接服务端」合并：麦克风上云是另一个量级的同意。 */}
        {onServer ? (
          <TranscriptionConsentRow
            status={status}
            spec={specOf(TRANSCRIPTION_CONSENT_KEY)}
            onChange={(value) => void setValue(TRANSCRIPTION_CONSENT_KEY, { kind: "bool", value })}
          />
        ) : null}
      </div>

      {/* 一次性交接提示（ADR-059 第 4 条）：名单与「问没问过」都由 Rust 算。它要你当场
          做个决定，故紧跟设置带、排在页脚之前。 */}
      {status.pending_handover.length > 0 ? (
        <div className="luna-compute-card__handover" role="note">
          <p className="luna-compute-card__handover-title">
            {t("compute_card.handover.title", { count: status.pending_handover.length })}
          </p>
          <p>{t("compute_card.handover.body")}</p>
          <div className="luna-compute-card__actions">
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={() => void resolveHandover(true)}
            >
              {t("compute_card.handover.accept")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => void resolveHandover(false)}
            >
              {t("compute_card.handover.keep")}
            </button>
          </div>
        </div>
      ) : null}

      {/* 判据是这条能力的落点，不是这台机器的落点：官方云用户没开转写同意时，
          转写的主力就是本机 tiny（ADR-126 第 2 条）。给两条出路，云的那条排前面
          ——它就在同一张卡上，比下载一个 1.5 GB 的模型近得多（ADR-127 第 5 条）。 */}
      {transcriptionRoute?.location === "device" && transcriptionCandidate?.is_minimal ? (
        <p className="luna-compute-card__hint">
          {t(onServer ? "compute_card.minimal_model_hint" : "compute_card.minimal_model_hint_local")}
        </p>
      ) : null}

      {/* 下一步只摆此刻有意义的那一颗：严格本地回隐私档；未登录去账户页（ADR-155）。 */}
      {strictLocal || signedOut ? (
        <div className="luna-compute-card__actions">
          {strictLocal ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => goTo(STRICT_LOCAL_ROUTE)}
            >
              {t("compute_card.action.strict_local")}
            </button>
          ) : null}
          {signedOut ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => goTo(ACCOUNT_ROUTE)}
            >
              {t("compute_card.action.sign_in")}
            </button>
          ) : null}
        </div>
      ) : null}

      {/* ── 页脚带：说明在左、动作在右 ──────────────────────────────── */}
      <ComputeSelfCheck />
    </section>
  );
}


export function ComputeCard(props: ComputeCardProps & { showingAdvanced?: boolean }) {
  // 走共享订阅：同屏几张面各取一次会停在不同的快照上。
  const status = useComputeStatus();
  const account = useAccount();
  const server = account?.server ?? null;

  const resolveHandover = useCallback(async (handOver: boolean) => {
    await commands.computeResolveHandover(handOver);
    await refreshComputeStatus();
  }, []);

  if (!status) return null;
  return (
    <ComputeCardView
      {...props}
      status={status}
      resolveHandover={resolveHandover}
      serverName={server ? (server.display_name ?? server.origin) : null}
    />
  );
}
