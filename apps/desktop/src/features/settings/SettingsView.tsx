// 设置中心主区（TDD-07 §4.1b / PRD-09 §5、ADR-120）：一层平铺，**一次只呈现一页**——
// 页头 + 分节行卡片 + 页专属面板；页面导航在主侧栏（SettingsNav）。
// 零业务：控件形态、选项、范围随规格从 Rust 注册表下发；校验/持久化全在 Rust（ADR-003）。

import {
  Fragment,
  useEffect,
  useRef,
  type ComponentType,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { useStatusVisitMetrics } from "../../hooks/use-status-visit-metrics";
import type { StatusVisitMetrics } from "../../hooks/use-status-visit-metrics";
import { SETTINGS_PAGE_DESCRIPTION_INLINE, type SettingSpecDto } from "../../ipc/bindings";
import { requestOnboarding } from "../../lib/app-events";
import { ComputeCard } from "./ComputeCard";
import { ModelLibraryPanel } from "./ModelLibraryPanel";
import { ModelAssignmentsPanel } from "./ModelAssignmentsPanel";
import { UsageExecutionPanel } from "./UsageExecutionPanel";
import { SemanticIndexPanel } from "./SemanticIndexPanel";
import { GlossaryPanel } from "./GlossaryPanel";
import { LocalKnowledgeMetricsPanel } from "./LocalKnowledgeMetricsPanel";
import { VaultIndexPanel } from "./VaultIndexPanel";
import { OrphanAssetsPanel } from "./OrphanAssetsPanel";
import { ExcludedPathsPanel } from "./ExcludedPathsPanel";
import { DataFootprintPanel } from "./DataFootprintPanel";
import { SettingsBackupPanel } from "./SettingsBackupPanel";
import { LocalWorkspaceMetricsPanel } from "./LocalWorkspaceMetricsPanel";
import { AppReleasePanel } from "./AppReleasePanel";
import { BuildInfoPanel } from "./BuildInfoPanel";
import { ThemeLibraryPanel } from "./ThemeLibraryPanel";
import { AppearancePreviewPanel } from "./AppearancePreviewPanel";
import { type SetValue } from "./SettingControls";
import {
  advancedSearchCount,
  panelRoute,
  searchPages,
  searchSpecs,
  sectionsOfPage,
} from "./settings-filter";
import { useSettingSearchText } from "./use-setting-search-text";
import { SettingField } from "./SettingField";
import { SettingsFieldContext } from "./settings-field-context";
import { parseSettingsRoute, type SettingsPageId } from "./settings-information-architecture";
import type { SettingsController } from "./use-settings-controller";
import { goTo } from "../../lib/navigate";
import { scrollWithinPane } from "../../lib/scroll-within-pane";
import { useResetSettingsPage } from "./use-reset-settings-page";

/**
 * **整页由一个外部组件承载**的页（账户）的组件表。表由 `app/` 组装后经 props 注入：
 * 账户面板属于另一个 feature，跨 feature 组装归 `app/`（TDD-15 §2.2），设置中心不反向依赖它。
 */
export type SettingsPagePanels = Partial<Record<SettingsPageId, ComponentType>>;

/** 页专属面板的渲染上下文：全部来自控制器与深链，面板自己不读全局。 */
interface PagePanelContext {
  captionPreview?: ReactNode;
  setValue: SetValue;
  /** 完整投影由持久化高级模式控制。 */
  showingAdvanced: boolean;
  /** 深链目标键/分节（`settings://<页>/<key>` 的尾段）。 */
  focusKey: string | null;
  /**
   * 取某个键的注册表声明。
   *
   * 面板承载的键要渲染的是**注册表声明的控件形态**（ADR-122 ⑤），故面板必须拿得到 spec；
   * 由宿主传入而不是面板自己去拉，既少一次 IPC，也让面板在首帧就有控件可渲染。
   */
  specOf: (key: string) => SettingSpecDto | undefined;
  /**
   * 「执行记录」那一面的到访观测（PRD-07-070）。
   *
   * **宿主在这一层，不在那张卡自己身上**：上报只认显式 open → closed，而卡片随页面
   * 条件渲染、切走即卸载，自己报不了终点。它此前的开合是那张浮层；浮层随 ADR-126
   * 第 7 条退役之后，"打开这一面"就是"来到这一页"，终点是切走这一页。
   * `interacted` 的判据不变——仍由摊在卡片内容上的那组捕获监听决定。
   */
  usageVisit: StatusVisitMetrics;
}

/**
 * 一页在设置行之外的内容：`lead` 渲在全部行卡片之前，`trail` 渲在全部行卡片之后，
 * `appendRows` 追加到最后一张行卡片里（它是"一行动作"，外壳归 `<ul>`，不是面板）。
 *
 * **`afterSection` 是给"属于某一节"的面板准备的**（键 = 注册表里的 `section` id）：
 * 一页多节时 `trail` 会把面板甩到整页末尾，于是同一个概念在一页里出现两处——「系统」页
 * 的「外部工具」正是这样被劈开过：分节下只剩「下载代理」一行，而工具面板隔着「实验室」
 * 与「诊断」落在页尾。该节在当前投影下不存在时（整节被滤掉），它挂的面板也不渲染。
 */
interface PagePanels {
  lead?: (ctx: PagePanelContext) => ReactNode;
  afterSection?: Record<string, (ctx: PagePanelContext) => ReactNode>;
  trail?: (ctx: PagePanelContext) => ReactNode;
  appendRows?: (ctx: PagePanelContext) => ReactNode;
}

/**
 * 页专属面板登记面（**唯一**一处）：它们的状态与动作不是"一个键的值"，塞进设置行会
 * 逆转设置页"按 kind 分发渲染"的通用性。每页在自己的位置渲染自己的面板——
 * 一个面板只会在它归属的那一页出现一次。
 *
 * 登记写成 `<页>: { … <XxxPanel /> }` 这一固定形状：`settings-panels.test.ts` 与
 * `scripts/check-settings-registry.py` §9 都按它解析（面板型页必须在此有面板；
 * 每个面板根元素必须用 `.luna-settings-panel` 唯一外壳）。
 */
const PAGE_PANELS: Partial<Record<SettingsPageId, PagePanels>> = {
  appearance: {
    afterSection: {
      interface: ({ setValue, specOf }) => (
        <>
          <AppearancePreviewPanel />
          <ThemeLibraryPanel spec={specOf("appearance.theme_family")} setValue={setValue} />
        </>
      ),
    },
  },
  translate: {
    lead: ({ specOf }) => <TranslationDirectionPreview specOf={specOf} />,
    afterSection: {
      caption_appearance: ({ captionPreview }) => captionPreview,
    },
  },
  general: {
    appendRows: ({ showingAdvanced }) => showingAdvanced ? <OnboardingRerunRow /> : null,
    // 「关于与更新」标准视图也在（PRD-09-051）：版本与检查更新是每个人都会找的东西。
    trail: ({ showingAdvanced }) => (
      <>
        <AppReleasePanel />
        {showingAdvanced ? (
          <>
            <SettingsBackupPanel />
            <BuildInfoPanel />
          </>
        ) : null}
      </>
    ),
  },
  ai_services: {
    // 「AI 在哪跑」在**设置行之前**：它是这一页的答案，其余各项都是它的细节。放在行后面
    // 就成了"看完八个开关再告诉你结论"，而普通用户在第一屏就该看懂。登录与换服务器不在
    // 这一页——它们只在账户页（ADR-155），算力卡上只写「由谁提供」并指过去。
    lead: ({ setValue, specOf, showingAdvanced }) => (
      <ComputeCard setValue={setValue} specOf={specOf} showingAdvanced={showingAdvanced} />
    ),
    // 执行记录留在 AI 服务页；本地模型独立为用户任务页，避免把算力落点与安装维护混在一起。
    trail: ({ focusKey, specOf, setValue, usageVisit, showingAdvanced }) => (
      <>
        {showingAdvanced ? <ModelAssignmentsPanel /> : null}
        <UsageExecutionPanel
          advancedMode={showingAdvanced}
          focusKey={focusKey}
          specOf={specOf}
          setValue={setValue}
          visit={usageVisit}
        />
      </>
    ),
  },
  local_models: {
    trail: ({ showingAdvanced, focusKey }) => (
      <ModelLibraryPanel advancedMode={showingAdvanced} focusKey={focusKey} />
    ),
  },
  glossary: {
    trail: () => <GlossaryPanel />,
  },
  // 语义索引的状态与建库入口（PRD-16-028）：「建库」是任务不是设置项。
  notes: {
    trail: ({ showingAdvanced }) => (
      <>
        <VaultIndexPanel />
        <OrphanAssetsPanel />
        {/* 排除路径（V4a）属高级层：入口在文件树右键，这里是总览与撤销处。 */}
        {showingAdvanced && <ExcludedPathsPanel />}
        <SemanticIndexPanel advancedMode={showingAdvanced} />
      </>
    ),
  },
  privacy: {
    // 两份本机记录并排呈现：用户问的是同一个问题——"这台机器上记了我什么"。
    // 它们的开关、数据与清除边界各自独立（TDD-12 §2.1）。
    // 笔记索引的重建入口（PRD-07-023）排在最前：它回答的是"我的东西为什么没显示出来"，
    // 比"这台机器上记了我什么"更常被找。
    //
    // 留在标准投影的四个各有理由，不是"顺手都放着"：笔记索引是**错误修复入口**；两份
    // 本机记录由标准投影里的隐私开关控制（开得了却看不见、清不掉，等于同意了一件事却
    // 没有反悔的地方）；数据足迹回答"这台机器上有我什么"，本身就是隐私透明度。
    // 判据 C 逐条成立（PRD-09-072），故它们**不**随「显示完整设置」隐去。
    trail: () => (
      <>
        <LocalKnowledgeMetricsPanel />
        <LocalWorkspaceMetricsPanel />
        {/* 设置备份是**搬机器用的工具**，不是设置也不是安全边界：不用它不会出错，多数人
            一次都不会用（判据 A/B/C 全不成立）。Vault 本身是一棵文件树，永远拿得到。 */}
        <DataFootprintPanel />
      </>
    ),
  },
};

/** 「重新运行引导」入口（PRD-14-060）：以覆盖层重开引导，不卸载工作区、不清任何状态——
 * 三态由真实条件派生，已完成的步骤自然不会再被强制。 */
function OnboardingRerunRow() {
  const { t } = useTranslation("settings");

  return (
    <li className="luna-settings-field">
      <div className="luna-settings-field__label">
        <span className="luna-settings-field__title">{t("onboarding_rerun.label")}</span>
        <p className="luna-settings-field__desc">{t("onboarding_rerun.desc")}</p>
      </div>
      <div className="luna-settings-field__control">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => {
            requestOnboarding();
          }}
        >
          {t("onboarding_rerun.action")}
        </button>
      </div>
      <div className="luna-settings-field__action" />
    </li>
  );
}

/**
 * 搜索命中了一整页（个人词典 / 账户：注册表零键，给不出任何一行）：给「去那儿」。
 *
 * 与 `PanelSettingHit` 共用同一张卡片形态——同一件事（"你要的东西在别处，点这儿过去"）
 * 在一页里有两种长相没有道理。
 */
function PageHit({ page }: { page: SettingsPageId }) {
  const { t } = useTranslation("settings");

  return (
    <li className="luna-settings-field luna-settings-field--panel">
      <div className="luna-settings-field__label">
        <span className="luna-settings-field__title">{t(`group.${page}`)}</span>
        <p className="luna-settings-field__desc">{t(`group_description.${page}`)}</p>
      </div>
      <div className="luna-settings-field__control">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => {
            goTo(`settings://${page}`);
          }}
        >
          {t("search.open_page")}
        </button>
      </div>
      <div className="luna-settings-field__action" />
    </li>
  );
}

/**
 * 搜索命中了一个由面板承载的键：给「去那儿」，而不是把控件复制一份到这里。
 */
function PanelSettingHit({ spec, route }: { spec: SettingSpecDto; route: string }) {
  const { t } = useTranslation("settings");
  const description = t(spec.desc_i18n_key, { defaultValue: "" });

  return (
    <li className="luna-settings-field luna-settings-field--panel">
      <div className="luna-settings-field__label">
        <span className="luna-settings-field__title">
          {t(spec.i18n_key)}
          <span className="luna-settings-field__chip">{t(`group.${spec.group}`)}</span>
        </span>
        {description ? <p className="luna-settings-field__desc">{description}</p> : null}
      </div>
      <div className="luna-settings-field__control">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => {
            goTo(route);
          }}
        >
          {t("search.open_panel")}
        </button>
      </div>
      <div className="luna-settings-field__action" />
    </li>
  );
}


/** 一行设置：标签 + 说明 + 状态徽标 + 控件（+ 仅在已改动时出现的「恢复默认」）。 */

/**
 * 当前页的正文：lead 面板 + 分节行卡片 + trail 面板。
 *
 * 一层平铺之后**没有"分组块"这一层**：页头（`<h1>` + 页说明）已经说了这一页是什么，
 * 页内再贴一遍组名就是同屏重复。此前那层 `GroupBlock` 连同它的 `titled`（"大类下只有
 * 一个分组时不贴组名"）随大类层一并退役。
 */
function PageBody({
  page,
  controller,
  panelContext,
}: {
  page: SettingsPageId;
  controller: SettingsController;
  panelContext: PagePanelContext;
}) {
  const { t } = useTranslation("settings");
  const sections = sectionsOfPage(controller.specs, page);
  const panels = PAGE_PANELS[page];

  return (
    <>
      {panels?.lead?.(panelContext)}
      {sections.filter((block) => block.section !== null).length > 1 ? (
        <nav className="luna-settings__section-links" aria-label={t("sectionNavigation")}>
          {sections.filter((block) => block.section !== null).map((block) => (
            <a key={block.section} href={`#settings-section-${block.section}`} onClick={(event) => {
              event.preventDefault();
              const target = document.getElementById(`settings-section-${block.section}`);
              if (target) {
                scrollWithinPane(target);
                target.focus({ preventScroll: true });
              }
            }}>{t(`section.${page}.${block.section}`)}</a>
          ))}
        </nav>
      ) : null}
      {/* 按页内分节渲染（PRD-09-070 ⑦）：每节一张行卡片；只有一节时不贴节标题——标签的
          职责是区分，没有可区分对象时它只是噪声（与文件树段头同一裁决）。
          零设置行时**不渲染 <ul>**：它自带边框与背景，空着就是一条无内容的空带。 */}
      {sections.map((block, index) => {
        const titled = sections.length > 1 && block.section !== null;
        // 锚点与面板分节同一形状（`#settings-section-<尾段>`），故 `settings://<页>/<节>`
        // 能滚到行分节，而不是只有面板分节才认这个深链。
        const sectionId = block.section ? `settings-section-${block.section}` : undefined;
        const sectionPanel = block.section ? panels?.afterSection?.[block.section] : undefined;
        return (
          <Fragment key={block.section ?? "all"}>
            <section
              id={sectionId}
              className="luna-settings-section"
              aria-labelledby={titled ? `${sectionId}-title` : undefined}
              tabIndex={-1}
            >
              {titled ? (
                <h3 id={`${sectionId}-title`} className="luna-settings-section__title">
                  {t(`section.${page}.${block.section}`)}
                </h3>
              ) : null}
              <ul className="luna-settings-group__items">
                {block.items.map((spec) => (
                  <SettingField key={spec.key} spec={spec} controller={controller} />
                ))}
                {index === sections.length - 1 ? panels?.appendRows?.(panelContext) : null}
              </ul>
            </section>
            {sectionPanel?.(panelContext)}
          </Fragment>
        );
      })}
      {panels?.trail?.(panelContext)}
    </>
  );
}

/**
 * 已经切过页的深链对象（每次导航外壳都给一个新对象，见 `openSettingsRoute`）。放在模块级而不是
 * 组件里：离开设置再回来时视图会重新挂载，组件内的记录随之丢失，旧深链就又切一次页。
 * 深链对象仍留在外壳里——「目标在完整设置里」的提示与切换后的重新定位都要用它。
 */
const NAVIGATED_FOCUS = new WeakSet<object>();

function TranslationDirectionPreview({ specOf }: Pick<PagePanelContext, "specOf">) {
  const { t } = useTranslation("settings");
  const native = specOf("popup_translate.native_lang")?.current_value;
  const foreign = specOf("popup_translate.default_foreign")?.current_value;
  if (native?.kind !== "str" || foreign?.kind !== "str") return null;
  return <p className="luna-settings__notice">{t("popup_translate.direction_preview", {
    native: t(`common:lang.${native.value}`, { defaultValue: native.value }),
    foreign: t(`common:lang.${foreign.value}`, { defaultValue: foreign.value }),
  })}</p>;
}

export function SettingsView({
  controller,
  pagePanels,
  focus = null,
  captionPreview,
}: {
  controller: SettingsController;
  /** 整页由外部组件承载的页的组件表（由 `app/` 组装注入，见 `SettingsPagePanels`）。 */
  pagePanels: SettingsPagePanels;
  /** 字幕呈现由 app 组装注入，设置 feature 不反向依赖字幕 feature。 */
  captionPreview?: ReactNode;
  /** 深链定位（`settings://<页>[/<key-or-section>]`；nonce 让重复导航也重新定位）。 */
  focus?: { route: string; nonce: number } | null;
}) {
  const { t } = useTranslation("settings");
  const searchText = useSettingSearchText();
  const resetPage = useResetSettingsPage(controller);
  const { activePage, query } = controller;
  const searching = query.trim().length > 0;
  const filtering = searching || controller.modifiedOnly;
  const showingAdvanced = controller.advancedMode;
  const advancedSwitchId = "luna-settings-advanced";
  // 「执行记录」那一面的到访：它就在「AI 算力与模型」这一页上，故到访 = 停在这一页，
  // 终点 = 切走或进入搜索（见 `PagePanelContext.usageVisit`）。
  const usageVisit = useStatusVisitMetrics("usage", !filtering && activePage === "ai_services");
  const focusRoute = focus ? parseSettingsRoute(focus.route) : null;
  const hiddenTarget = !showingAdvanced && focusRoute
    ? controller.allSpecs.find((spec) => spec.available && spec.surface.kind !== "internal" && spec.group === focusRoute.page && spec.visibility === "advanced" && (spec.key === focusRoute.target || spec.section === focusRoute.target))
    : undefined;

  // 深链跳转：语义是"这一次导航"，故只依赖 focus 对象本身（每次导航都是新对象，
  // 含 nonce），控制器经 ref 取当前值，避免把它写进依赖。
  // 解析在 `parseSettingsRoute` 一处：未知首段（笔误、改名后残留的旧路由）直接忽略，
  // 不会让控制器落到一个不存在的页上。
  const controllerRef = useRef(controller);
  controllerRef.current = controller;
  useEffect(() => {
    const route = focus ? parseSettingsRoute(focus.route) : null;
    if (!route) return;
    const current = controllerRef.current;
    // 切页只属于「这一次导航」本身（2026-10-08 真机发现）：此前切完整设置、或离开设置再回来
    // （视图重新挂载），都会重放上一次深链，把已经走开的用户拉回那一页。规格首次载入前页面
    // 名单可能还是空的，那时不算切过。
    if (focus && !NAVIGATED_FOCUS.has(focus) && current.pages.includes(route.page)) {
      current.setQuery("");
      current.setModifiedOnly(false);
      current.selectPage(route.page);
      NAVIGATED_FOCUS.add(focus);
    }
    // React 先切页，再在下一帧定位真实元素；无尾段时仅打开该页（页本身就是落点，
    // 一层平铺之后不再需要"先开大类再滚到组"这一跳）。
    //
    // **一帧不够，要等到它真的出现**（真机 2026-09-12）：落点可能藏在一个刚被切过去的
    // 页签后面（算力卡的「更改」→ 模型库「模型分配」），而切页签发生在 effect 里、比这
    // 一帧晚。只试一次的表现是"点了停在原地"——目标在下方悄悄出现，用户看不出发生了
    // 什么，正是 ADR-125 第 14 条要修掉的那种形状。找到即停，找不到最多再等几帧。
    // **上限按时间给，不按帧数**（真机 2026-09-12 二次纠正）：帧数在 debug 构建与
    // 慢机器上换算出的时间差好几倍，而这条链要等的是"揭示高级投影 → 整页重渲染 →
    // 页签切换 → 那一行挂上"。头一版给 12 帧（约 200 ms），真机上第一次点「更改」
    // 落空、第二次才成——同一颗按钮时灵时不灵，比一直不灵更难被发现。
    const deadline = performance.now() + 1000;
    const locate = () => {
      if (!route.target) return;
      const escaped = CSS.escape(route.target);
      const element = document.querySelector<HTMLElement>(
        `[data-setting-key="${escaped}"], #settings-section-${escaped}`,
      );
      if (element) {
        scrollWithinPane(element);
        element.focus({ preventScroll: true });
        return;
      }
      // 有上限而不是无限重试：目标可能压根不存在（过期深链、改名后残留的旧路由），
      // 那时安静收手就是正确行为。
      if (performance.now() < deadline) window.requestAnimationFrame(locate);
    };
    window.requestAnimationFrame(locate);
  }, [focus, controller.allSpecs.length, showingAdvanced]);

  const results = (searching ? searchSpecs(controller.specs, query, searchText) : controller.specs)
    .filter((spec) => !controller.modifiedOnly || spec.modified);
  // 命中的是一整页时给一张「去这一页」的卡片：面板型页（个人词典 / 账户）注册表零键，
  // 遍历 specs 的搜索永远碰不到它们；按页名搜时答案本来也就是那一页。
  const pageResults = controller.modifiedOnly ? [] : searchPages(controller.pages, query, searchText);
  const hiddenAdvancedCount = showingAdvanced
    ? 0
    : advancedSearchCount(controller.allSpecs.filter((spec) => !controller.modifiedOnly || spec.modified), query, searchText);

  if (controller.loading || (!searching && activePage === "")) {
    // 规格未载入，或载入后还没落到任何一页（控制器下一帧即回落到首页）。
    return (
      <section className="luna-settings" aria-label={t("title")}>
        <p className="luna-empty">{t("title")}…</p>
      </section>
    );
  }

  // 整页由外部组件承载的页（账户）：页头与内建页同一枚（此前账户页自造页头，是全设置
  // 中心唯一长得不一样的一页）。
  const PagePanel =
    !filtering && activePage !== "" ? (pagePanels[activePage] ?? null) : null;
  const modifiedCount = activePage === "" ? 0 : (controller.modifiedByPage[activePage] ?? 0);
  const panelContext: PagePanelContext = {
    captionPreview,
    setValue: controller.setValue,
    showingAdvanced,
    focusKey: focusRoute?.target ?? null,
    usageVisit,
    // 面板与列表消费同一显示投影，不能通过自取完整快照泄露高级编辑入口。
    specOf: (key) => controller.allSpecs.find((spec) => spec.key === key && spec.available && (showingAdvanced || spec.visibility === "standard")),
  };

  return (
    <SettingsFieldContext.Provider value={controller}>
    <section className="luna-settings" aria-label={t("title")}>
      {controller.error ? (
        <p className="luna-field-error" role="alert">
          {controller.error === "SETTING_RESET_STALE" ? t("error.reset_stale") : controller.error}
        </p>
      ) : null}

      <header className="luna-settings__header">
        <div className="luna-settings__heading">
          <h1>{controller.modifiedOnly ? t("search.modified_title") : searching ? t("search.results_title") : t(`group.${activePage}`)}</h1>
          {filtering || (activePage !== "" && SETTINGS_PAGE_DESCRIPTION_INLINE[activePage]) ? <p>
            {searching
              ? t("search.results_desc", {
                  count: results.length + pageResults.length,
                  query: query.trim(),
                })
              : controller.modifiedOnly ? t("search.modified_desc", { count: results.length }) : t(`group_description.${activePage}`)}
          </p> : null}
        </div>
        <div className="luna-settings__header-actions">
          <button
            type="button"
            role="switch"
            aria-checked={controller.advancedMode}
            aria-labelledby={`${advancedSwitchId}-title`}
            aria-describedby={`${advancedSwitchId}-desc`}
            className="luna-settings__advanced"
            title={t("advanced_mode_desc")}
            onClick={(event) => {
              event.currentTarget.focus();
              void controller.setAdvancedMode(!controller.advancedMode);
            }}
          >
            <span id={`${advancedSwitchId}-title`} className="luna-settings__advanced-title">
              {t("advanced_mode")}
            </span>
            <span className="luna-switch__track" aria-hidden="true">
              <span className="luna-switch__thumb" />
            </span>
          </button>
          <span id={`${advancedSwitchId}-desc`} className="luna-visually-hidden">
            {t("advanced_mode_desc")}
          </span>
          {/* 「改动要不要保存」是用户真会问的问题（尤其没有确定/取消按钮时）：在页头讲一次，
              而不是逐行贴标记；写入成功时行尾还会闪一次对勾作为即时回执。 */}
          {filtering || PagePanel ? null : (
            <span className="luna-settings__autosave">{t("autosave_hint")}</span>
          )}
          {filtering ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => {
                controller.setQuery("");
                controller.setModifiedOnly(false);
              }}
            >
              {t(controller.modifiedOnly ? "search.clear_filters" : "search.clear")}
            </button>
          ) : modifiedCount > 0 && activePage !== "" ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => {
                void resetPage(activePage);
              }}
            >
              {t(
                    controller.advancedMode
                      ? "action.reset_group_all"
                      : "action.reset_group_standard",
                  )}
            </button>
          ) : null}
        </div>
      </header>

      {/* 查无：只有完整设置里也没有时才说「没有匹配」——高级设置里有命中时那句话不成立，
          且与标题下的「0 项匹配」同屏两遍（真机 2026-10-06：侧栏、标题、正文三遍）。 */}
      {filtering && results.length === 0 && pageResults.length === 0 && hiddenAdvancedCount === 0 ? (
        <p className="luna-empty">{controller.modifiedOnly ? t("search.modified_empty") : t("search.empty", { query: query.trim() })}</p>
      ) : null}

      {hiddenTarget ? <p className="luna-settings__notice" role="status">{t("advanced_target", { item: t(hiddenTarget.i18n_key), page: t(`group.${hiddenTarget.group}`) })}</p> : null}
      {/* 标准模式保留高级命中数，并给一步到位的入口（VS Code / 系统设置的搜索同法）：此前只是
          一行灰字，用户得自己去找「显示完整设置」那颗开关。 */}
      {searching && hiddenAdvancedCount > 0 ? (
        <p className="luna-settings__more-advanced">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => {
              void controller.setAdvancedMode(true);
            }}
          >
            {t("search.show_advanced", { count: hiddenAdvancedCount })}
          </button>
        </p>
      ) : null}

      {/* 搜索结果保持扁平（命中项各标所属页）。搜索命中了一个由面板承载的键：给「去那儿」，
          而不是把控件复制一份到这里——这些键的写入口在面板上（模型分配的唯一写入命令是
          `assign_model`），在搜索结果里再渲染一个控件要么写不进去，要么变成第二个入口。 */}
      {filtering && (results.length > 0 || pageResults.length > 0) ? (
        <ul className="luna-settings-group__items">
          {pageResults.map((page) => (
            <PageHit key={page} page={page} />
          ))}
          {results.map((spec) => {
            const route = panelRoute(spec);
            return route ? (
              <PanelSettingHit key={spec.key} spec={spec} route={route} />
            ) : (
              <SettingField key={spec.key} spec={spec} controller={controller} showPage />
            );
          })}
        </ul>
      ) : null}

      {PagePanel ? <PagePanel /> : null}

      {!filtering && !PagePanel && activePage !== "" ? (
        <PageBody page={activePage} controller={controller} panelContext={panelContext} />
      ) : null}
    </section>
    </SettingsFieldContext.Provider>
  );
}
