/**
 * 「应用」目的地（ADR-165 ⑤ / ADR-168 / PRD-18-005 / PRD-19-036）：活动栏贴底系统区的一格。
 *
 * 两态：页签页（「应用」列出全部一方应用、「已安装」只列装了的，`apps-route.ts`）与详情页（一个
 * 应用：做什么、启用后会多出什么、权限与数据去向）。路由 `apps://[<页签词>|<id>]` 由宿主解析后把
 * `selected` 交进来。
 *
 * **状态与主按钮都由 Rust 给出**（ADR-168 ①：`AppDto.state` / `AppDto.action`），列表行与详情页
 * 共用同一份动作（`useAppActions`）：在列表里点「安装」与在详情页点，走的是同一条路、同一张确认框。
 *
 * **清单与判定都在 Rust**（`apps_list`）：「启用后会多出」「权限与数据」逐条来自清单里的
 * 贡献种类与权限，文案按 `apps.<id>.contribution.<kind>` / `apps.<id>.permission.<kind>`
 * 取——这里不写「创作有哪些入口」。卸载与暂时停用只收入口、不碰库（ADR-165 ④）。
 * 详情页按应用商店的信息架构（ADR-168 ⑥）：亮点 → 关于（入口折进「在哪里找到它」）→ 权限与
 * 隐私，右侧信息栏。刚装上时顶上出现这个应用自己的入门（`AppSetup`，按 id 穷尽，都可跳过）。
 * 带组件的应用（ADR-167 ⑤，如「从链接导入」）的组件不进默认视图，在「开源组件与许可证」二级页
 * （`apps://<id>/components`），由那个应用自己的组件面板承载（`AppComponents`）。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  commands,
  events,
  type AppDto,
  type AppLifecycleDto,
  type ContentCatalogDto,
  type TemplatePackDto,
} from "../../ipc/bindings";
import { TaskStatus, type TaskStatusKind } from "../../components/TaskStatus";
import { EmptyState } from "../../components/ui/EmptyState";
import { FactRow } from "../../components/ui/FactRow";
import { HudActionMenu, type HudActionMenuItem } from "../../components/ui/HudActionMenu";
import { ChevronRightIcon } from "@lunarca/editor-core/Icon";
import { InlineDetails } from "../../components/ui/InlineDetails";
import { checkAppUpdates, refreshApps, useApps } from "../../hooks/use-apps";
import { useBoolSetting } from "../../hooks/use-bool-setting";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { useTemplateMarket } from "../../hooks/use-template-market";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { goTo } from "../../lib/navigate";
import { useThemeLibrary } from "../../theme/use-theme-library";
import { APPS_TABS, appComponentsRoute, appsRouteOf, appsTabRoute, type AppsTab } from "./apps-route";
import { appUi } from "./app-registry";
import { InstalledTemplatePacks } from "./InstalledTemplatePacks";
import { InstalledThemes, installedThemeCounts } from "./InstalledThemes";
import { TemplatesTab } from "./TemplatesTab";
import { ThemesTab } from "./ThemesTab";
import { useAppActions, type AppActions } from "./use-app-actions";

export interface AppsViewProps {
  /** `apps://` 之后那一段（页签词或应用 id）；`null` = 「应用」页签。 */
  selected: string | null;
}

/** 应用图标（`APP_UI`，ADR-169 ⑧）。 */
function AppIcon({ id }: { id: string }) {
  return <>{appUi(id)?.icon(22) ?? null}</>;
}

/** 应用的组件面板（`APP_UI`）；没有组件的应用不渲染。 */
function AppComponents({ id, advancedMode }: { id: string; advancedMode: boolean }) {
  const Panel = appUi(id)?.Components;
  return Panel ? <Panel advancedMode={advancedMode} /> : null;
}

/** 严格本地拦下联网时的稳定错误码（与抓取同一个，`luna_fetch::FetchError::StrictLocalBlocked`）。 */
const STRICT_LOCAL_BLOCKED = "STRICT_LOCAL_BLOCKED";

/** 这个应用有没有刚装上后的入门：从列表行装上的据此带去详情页。 */
function hasSetup(id: string): boolean {
  return appUi(id)?.Setup != null;
}

/** 刚启用后的入门（`APP_UI`）；没有的应用不渲染。 */
function AppSetup({ id, onDone }: { id: string; onDone: () => void }) {
  const Setup = appUi(id)?.Setup;
  return Setup ? <Setup onDone={onDone} /> : null;
}

/** 状态标签的样子（文案按 `apps.status.<state>` 取）。 */
const STATUS_KIND: Record<AppLifecycleDto, TaskStatusKind> = {
  not_installed: "idle",
  installed: "completed",
  paused: "idle",
  needs_repair: "needs_action",
  update_available: "needs_action",
};

function AppStatus({ app }: { app: AppDto }) {
  const { t } = useTranslation("workspace");
  return <TaskStatus kind={STATUS_KIND[app.state]} label={t(`apps.status.${app.state}`)} />;
}

/** 操作结果的提示（行内与详情页同一种写法）。 */
function ActionNotice({ actions }: { actions: AppActions }) {
  const { t } = useTranslation("workspace");
  if (actions.notice === null) return null;
  return (
    <p className={actions.failed ? "luna-apps__error" : "luna-apps__note"} role={actions.failed ? "alert" : "status"}>
      {t(actions.notice)}
    </p>
  );
}

/**
 * 列表里的一行（商店的写法）：左边点进详情，右边是这一态的主按钮。没装的写安装要占多大；装了的
 * 写状态（「已安装」页签另写版本）。
 */
function AppRow({ app, showVersion }: { app: AppDto; showVersion: boolean }) {
  const { t, i18n } = useTranslation("workspace");
  const actions = useAppActions(app, {
    onEnabled: hasSetup(app.id) ? () => goTo(`apps://${app.id}`) : undefined,
  });
  const meta = [
    showVersion ? t("apps.meta.version", { version: app.version }) : null,
    app.size_bytes !== null ? t("apps.meta.size", { size: formatBytes(app.size_bytes, i18n.language) }) : null,
  ].filter((part): part is string => part !== null);
  return (
    <li className="luna-apps__row">
      <button type="button" className="luna-apps__row-main" onClick={() => goTo(`apps://${app.id}`)}>
        <span className="luna-apps__icon" aria-hidden="true">
          <AppIcon id={app.id} />
        </span>
        <span className="luna-apps__card-text">
          <span className="luna-apps__card-name">{t(`apps.${app.id}.name`)}</span>
          <span className="luna-apps__card-summary">{t(`apps.${app.id}.summary`)}</span>
          {meta.length > 0 ? <span className="luna-apps__meta">{meta.join(" · ")}</span> : null}
        </span>
      </button>
      <span className="luna-apps__row-side">
        {app.state !== "not_installed" ? <AppStatus app={app} /> : null}
        {actions.primary ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={actions.busy}
            aria-label={t("apps.rowAction", { action: actions.primary.label, name: t(`apps.${app.id}.name`) })}
            onClick={actions.primary.run}
          >
            {actions.progressLabel ?? actions.primary.label}
          </button>
        ) : (
          <ChevronRightIcon size={14} />
        )}
      </span>
      <ActionNotice actions={actions} />
    </li>
  );
}

/** 检查更新这一次走到哪了（只在这一页上说，不落盘）。查完之后说的是**此刻**有几个可更新——不是
 * 查的那一刻：更新完一个，这句话要跟着变（真机发现：更新完仍写「1 个应用可更新」）。 */
type UpdateCheck =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "checked" }
  | { kind: "failed"; reason: "strictLocal" | "trustChanged" | "failed" };

/**
 * 「应用」页头的「检查更新」（ADR-168 ⑧ / ADR-170 ③）：整个「应用」只有这一个手动入口——用户点了才联网，一次查完所有带
 * 组件的应用与主题、模板两份目录（`check_content_updates`，刷新是 `Catalogs::refresh` 一个组动作，顺带模型与提示词
 * 目录，成败由 Rust 按所问目录各自判）；不后台检查、不自动更新。查完「可更新」的应用行、主题行与模板包行上出现「更新」。
 * 内容那一半只在在线内容目录可用时才查（Rust 的 `can_check_updates`）。结论写在页头下方，各页签共用。
 */
function useUpdatesCheck({
  updatable,
  disabled,
  apps,
  catalogs,
}: {
  updatable: number;
  /** 被封禁、已停用的主题数：查完的结论要点出它们，不能只说「都是最新版本」（2026-10-23 Windows 真机）。 */
  disabled: number;
  apps: boolean;
  /** 要查的内容目录；空 = 不查内容。 */
  catalogs: ContentCatalogDto[];
}) {
  const { t } = useTranslation("workspace");
  const [check, setCheck] = useState<UpdateCheck>({ kind: "idle" });
  const run = async () => {
    setCheck({ kind: "checking" });
    const [appResult, catalogResult] = await Promise.all([
      apps ? checkAppUpdates() : null,
      catalogs.length > 0 ? commands.checkContentUpdates(catalogs) : null,
    ]);
    const appError = appResult !== null && !appResult.ok ? appResult.error : null;
    const catalogError = catalogResult !== null && catalogResult.status === "error" ? catalogResult.error : null;
    setCheck(
      appError === null && catalogError === null
        ? { kind: "checked" }
        : {
            kind: "failed",
            reason:
              appError?.startsWith(STRICT_LOCAL_BLOCKED) === true || catalogError?.startsWith(STRICT_LOCAL_BLOCKED) === true
                ? "strictLocal"
                : catalogError?.startsWith("CONTENT_TRUST_CHANGED") === true
                  ? "trustChanged"
                  : "failed",
          },
    );
  };
  const status =
    check.kind === "checked"
      ? disabled > 0
        ? updatable > 0
          ? t("apps.updates.availableWithDisabled", { count: updatable, disabled })
          : t("apps.updates.upToDateWithDisabled", { disabled })
        : updatable > 0
          ? t("apps.updates.available", { count: updatable })
          : t("apps.updates.upToDate")
      : check.kind === "failed"
        ? t(`apps.updates.${check.reason}` as const)
        : t("apps.updates.hint");
  return { check, run, status };
}

function AppsHome({ tab, apps }: { tab: AppsTab; apps: readonly AppDto[] }) {
  const { t } = useTranslation("workspace");
  const navigation = useTablistNavigation(APPS_TABS, tab, (next) => goTo(appsTabRoute(next)));
  const library = useThemeLibrary();
  const { market: templateMarket, reload: reloadTemplateMarket } = useTemplateMarket();
  const [packs, setPacks] = useState<TemplatePackDto[]>([]);
  const reloadPacks = useCallback(() => {
    void commands.templatePacks().then(setPacks);
  }, []);
  useEffect(() => {
    reloadPacks();
    // 模板目录换了一份：哪些包可更新跟着变（`update_to` 由 Rust 用同一份市场状态算）。
    const unlisten = events.templatesChanged.listen(() => reloadPacks());
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [reloadPacks]);
  const installed = apps.filter((app) => app.installed);
  const themeCounts = installedThemeCounts(library);
  const packUpdates = packs.filter((pack) => pack.update_to !== null).length;
  const installedCount = installed.length + themeCounts.count + packs.length;
  const updatable =
    installed.filter((app) => app.state === "update_available").length + themeCounts.updates + packUpdates;
  const checkApps = installed.some((app) => app.components.length > 0);
  // 能不能查由 Rust 判（`can_check_updates`：已登录、未开严格本地，主题与模板两份目录同一个前提），不看「获取更多」那一块
  // 在不在：目录条目全被封禁时那一块消失，而查一遍正是得知解除封禁的途径（2026-10-23 Windows 真机）。页头的入口问的是
  // 整个「应用」，所以两份目录一起问，不看装没装。
  const checkCatalogs: ContentCatalogDto[] = library.canCheckUpdates ? ["themes", "templates"] : [];
  const disabled = library.disabled.length + packs.filter((pack) => pack.disabled_reason !== null).length;
  const updates = useUpdatesCheck({ updatable, disabled, apps: checkApps, catalogs: checkCatalogs });
  const canCheck = checkApps || checkCatalogs.length > 0;
  // 打开即重验（ADR-170 ②）：先用缓存渲染，Rust 判过新鲜度（5 分钟）需要时在后台拉一遍；交付一变由
  // `themes://changed` / `templates://changed` 带出新内容。门控不放行时 Rust 什么都不发。
  useEffect(() => {
    void commands.revalidateCatalogs();
  }, []);
  const shown = tab === "installed" ? installed : apps;
  return (
    <section className="luna-apps" aria-labelledby="luna-apps-title">
      <header className="luna-main-header">
        <div className="luna-main-header__title">
          <h1 id="luna-apps-title">{t("nav.apps")}</h1>
        </div>
        {/* 只有带组件的已装应用与在线内容目录才有东西可查（一方应用本身随 Lunarca 更新）。 */}
        {canCheck ? (
          <div className="luna-main-header__actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={updates.check.kind === "checking"}
              onClick={() => void updates.run()}
            >
              {updates.check.kind === "checking" ? t("apps.updates.checking") : t("apps.updates.check")}
            </button>
          </div>
        ) : null}
      </header>
      {canCheck ? (
        <p
          className={updates.check.kind === "failed" ? "luna-apps__error" : "luna-apps__note"}
          role={updates.check.kind === "failed" ? "alert" : "status"}
        >
          {updates.status}
        </p>
      ) : null}
      <div className="luna-apps__tabs" role="tablist" aria-label={t("apps.tabs.label")}>
        {APPS_TABS.map((value) => (
          <button
            key={value}
            ref={navigation.register(value)}
            type="button"
            role="tab"
            id={`apps-tab-${value}`}
            aria-selected={tab === value}
            aria-controls="apps-tabpanel"
            tabIndex={navigation.tabIndexOf(value)}
            className="luna-apps__tab"
            onKeyDown={navigation.onKeyDown}
            onClick={() => goTo(appsTabRoute(value))}
          >
            {value === "installed" && installedCount > 0
              ? updatable > 0
                ? t("apps.tab.installedUpdates", { count: installedCount, updates: updatable })
                : t("apps.tab.installedCount", { count: installedCount })
              : t(`apps.tab.${value}`)}
          </button>
        ))}
      </div>
      <div id="apps-tabpanel" role="tabpanel" aria-labelledby={`apps-tab-${tab}`} className="luna-apps__panel">
        {tab === "themes" ? <ThemesTab /> : null}
        {tab === "templates" ? <TemplatesTab /> : null}
        {tab === "apps" ? <p className="luna-apps__lede">{t("apps.lede")}</p> : null}
        {tab === "themes" || tab === "templates" ? null : shown.length > 0 ? (
          <ul className="luna-apps__list">
            {shown.map((app) => (
              <AppRow key={app.id} app={app} showVersion={tab === "installed"} />
            ))}
          </ul>
        ) : apps.length > 0 && !(tab === "installed" && themeCounts.count + packs.length > 0) ? (
          <EmptyState
            icon={null}
            title={t("apps.installedEmpty.title")}
            description={t("apps.installedEmpty.description")}
            action={
              <button type="button" className="luna-btn luna-btn--secondary" onClick={() => goTo(appsTabRoute("apps"))}>
                {t("apps.installedEmpty.browse")}
              </button>
            }
          />
        ) : null}
        {tab === "installed" ? <InstalledThemes library={library} /> : null}
        {tab === "installed" ? (
          <InstalledTemplatePacks
            packs={packs}
            market={templateMarket}
            onChanged={() => {
              reloadPacks();
              reloadTemplateMarket();
            }}
          />
        ) : null}
      </div>
    </section>
  );
}

function AppDetail({ app }: { app: AppDto }) {
  const { t } = useTranslation("workspace");
  const actions = useAppActions(app);
  const name = t(`apps.${app.id}.name`);
  /** 「···」（ADR-168 ④）：暂时停用、开源组件与许可证、卸载（最后、危险色）。 */
  const moreItems: HudActionMenuItem[] = [
    ...(actions.pause ? [{ id: actions.pause.id, label: actions.pause.label, disabled: actions.busy, onSelect: actions.pause.run }] : []),
    ...(app.components.length > 0
      ? [{ id: "components", label: t("apps.components.title"), onSelect: () => goTo(appComponentsRoute(app.id)) }]
      : []),
    ...(actions.uninstall
      ? [
          {
            id: actions.uninstall.id,
            label: actions.uninstall.label,
            tone: "danger" as const,
            disabled: actions.busy,
            onSelect: actions.uninstall.run,
          },
        ]
      : []),
  ];

  return (
    <section className="luna-apps" aria-labelledby="luna-apps-detail-title">
      <header className="luna-main-header">
        <div className="luna-main-header__title">
          <Crumbs trail={[{ label: t("nav.apps"), route: "apps://" }]} />
          <h1 id="luna-apps-detail-title">{name}</h1>
          <AppStatus app={app} />
        </div>
        <div className="luna-main-header__actions">
          {actions.primary ? (
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={actions.busy}
              onClick={actions.primary.run}
            >
              {actions.progressLabel ?? actions.primary.label}
            </button>
          ) : null}
          {moreItems.length > 0 ? (
            <HudActionMenu label={t("apps.more")} density="sm" side="bottom" items={moreItems} />
          ) : null}
        </div>
      </header>
      <ActionNotice actions={actions} />
      <div className="luna-apps__detail">
        {app.enabled && actions.justEnabled ? <AppSetup id={app.id} onDone={actions.dismissSetup} /> : null}
        <div className="luna-apps__hero">
          <span className="luna-apps__icon luna-apps__icon--large" aria-hidden="true">
            <AppIcon id={app.id} />
          </span>
          <div className="luna-apps__hero-text">
            <p className="luna-apps__byline">{t("apps.byline", { developer: app.developer, license: app.license })}</p>
            <p className="luna-apps__summary">{t(`apps.${app.id}.summary`)}</p>
          </div>
        </div>
        <div className="luna-apps__layout">
          <div className="luna-apps__main">
            <ul className="luna-apps__highlights" aria-label={t("apps.highlights.label")}>
              {app.highlights.map((highlight) => (
                <li key={highlight} className="luna-apps__highlight">
                  <span className="luna-apps__highlight-title">{t(`apps.${app.id}.highlight.${highlight}.title`)}</span>
                  <span className="luna-apps__highlight-body">{t(`apps.${app.id}.highlight.${highlight}.body`)}</span>
                </li>
              ))}
            </ul>
            <section className="luna-apps__section" aria-labelledby="luna-apps-about">
              <h2 id="luna-apps-about">{t("apps.about.title")}</h2>
              <p className="luna-apps__description">{t(`apps.${app.id}.description`)}</p>
              <InlineDetails summary={t("apps.about.where")}>
                <ul className="luna-apps__facts">
                  {app.contributions.map((kind) => (
                    <li key={kind}>{t(`apps.${app.id}.contribution.${kind}`)}</li>
                  ))}
                </ul>
              </InlineDetails>
            </section>
            <section className="luna-apps__section" aria-labelledby="luna-apps-privacy">
              <h2 id="luna-apps-privacy">{t("apps.privacy.title")}</h2>
              <ul className="luna-apps__facts">
                <li>{t("apps.privacy.noCollection")}</li>
                {app.permissions.map((kind) => (
                  <li key={kind}>{t(`apps.${app.id}.permission.${kind}`)}</li>
                ))}
              </ul>
              {app.components.length > 0 ? <p className="luna-apps__note">{t(`apps.${app.id}.legal`)}</p> : null}
              <p className="luna-apps__note">{t("apps.disable.note")}</p>
            </section>
          </div>
          <AppInfo app={app} />
        </div>
      </div>
    </section>
  );
}

/** 信息栏（ADR-168 ⑥）：版本、大小、开发者、许可证、平台，带组件的应用多一条去组件页的路。 */
function AppInfo({ app }: { app: AppDto }) {
  const { t, i18n } = useTranslation("workspace");
  const size =
    app.size_bytes !== null
      ? t("apps.meta.size", { size: formatBytes(app.size_bytes, i18n.language) })
      : t(app.components.length > 0 ? "apps.info.sizeNothingExtra" : "apps.info.sizeIncluded");
  return (
    <aside className="luna-apps__info" aria-label={t("apps.info.label")}>
      <FactRow label={t("apps.info.version")} value={app.version} />
      <FactRow label={t("apps.info.size")} value={size} />
      <FactRow label={t("apps.info.developer")} value={app.developer} />
      <FactRow label={t("apps.info.license")} value={app.license} />
      <FactRow label={t("apps.info.platforms")} value={t("apps.info.platformsValue")} />
      {app.components.length > 0 ? (
        <button type="button" className="luna-apps__info-link" onClick={() => goTo(appComponentsRoute(app.id))}>
          {t("apps.components.title")}
          <ChevronRightIcon size={12} />
        </button>
      ) : null}
    </aside>
  );
}

/** 面包屑：「应用 › …」，返回只在这里（ADR-165 ⑨）。 */
function Crumbs({ trail }: { trail: readonly { label: string; route: string }[] }) {
  const { t } = useTranslation("workspace");
  return (
    <nav className="luna-apps__crumb" aria-label={t("apps.crumbLabel")}>
      {trail.map((step) => (
        <span key={step.route} className="luna-apps__crumb">
          <button type="button" className="luna-apps__crumb-link" onClick={() => goTo(step.route)}>
            {step.label}
          </button>
          <ChevronRightIcon size={12} />
        </span>
      ))}
    </nav>
  );
}

/**
 * 「开源组件与许可证」（ADR-168 ⑤）：应用要用到的第三方程序逐件写明——许可证、来源、在用的是
 * 哪一份、版本与位置；下面是那个应用自己的组件面板（逐件安装、更新、卸载与自行安装引导，改路径
 * 只在高级模式出现）。默认视图（详情页）不出现这些。
 */
function AppComponentsPage({ app }: { app: AppDto }) {
  const { t } = useTranslation("workspace");
  const advancedMode = useBoolSetting("settings.advanced_mode", false);
  const name = t(`apps.${app.id}.name`);
  return (
    <section className="luna-apps" aria-labelledby="luna-apps-components-title">
      <header className="luna-main-header">
        <div className="luna-main-header__title">
          <Crumbs
            trail={[
              { label: t("nav.apps"), route: "apps://" },
              { label: name, route: `apps://${app.id}` },
            ]}
          />
          <h1 id="luna-apps-components-title">{t("apps.components.title")}</h1>
        </div>
      </header>
      <p className="luna-apps__lede">{t("apps.components.hint")}</p>
      <ul className="luna-apps__list" aria-label={t("apps.components.listLabel", { name })}>
        {app.components.map((component) => (
          <li key={component.id} className="luna-apps__component">
            <p className="luna-apps__card-name">
              {t("apps.components.item", {
                name: t(`apps.components.name.${component.id}`),
                need: t(`apps.components.need.${component.need}`),
              })}
            </p>
            <FactRow label={t("apps.info.license")} value={component.license} />
            <FactRow label={t("apps.components.source")} value={component.source} />
            <FactRow label={t("apps.components.inUse")} value={t(`apps.components.presence.${component.presence}`)} />
            {component.version !== null ? (
              <FactRow label={t("apps.info.version")} value={component.version} />
            ) : null}
            {component.path !== null ? <FactRow label={t("apps.components.path")} value={component.path} /> : null}
          </li>
        ))}
      </ul>
      <section className="luna-apps__section" aria-labelledby="luna-apps-components-manage">
        <h2 id="luna-apps-components-manage">{t("apps.components.manage")}</h2>
        {app.installed ? (
          // 安装、修复、卸载也可能由详情页完成：状态一变就重挂面板，让它重新取一次工具状态。
          <AppComponents key={app.state} id={app.id} advancedMode={advancedMode} />
        ) : (
          // 没装的应用不在这里逐件装组件：装组件是「安装」这一件事的一部分，入口只有一个。
          <p className="luna-apps__note">{t("apps.components.installFirst", { name })}</p>
        )}
      </section>
    </section>
  );
}

export function AppsView({ selected }: AppsViewProps) {
  const { t } = useTranslation("workspace");
  const apps = useApps();
  const route = appsRouteOf(selected);
  // 每进一页重取一次：组件文件可能在应用外被删、被装（ADR-168 A1 真机发现），状态不能停在上次。
  useEffect(() => {
    refreshApps();
  }, [selected]);
  if (route.kind === "tab") return <AppsHome tab={route.tab} apps={apps} />;
  const app = apps.find((entry) => entry.id === route.id);
  if (app && route.kind === "components" && app.components.length > 0) return <AppComponentsPage app={app} />;
  if (app && route.kind === "detail") return <AppDetail app={app} />;
  // 清单还没取回时不先说「找不到」：取回前 `apps` 为空。
  if (apps.length === 0) return null;
  return (
    <section className="luna-apps">
      <EmptyState
        icon={null}
        title={t("apps.notFound.title")}
        description={t("apps.notFound.description")}
        action={
          <button type="button" className="luna-btn luna-btn--secondary" onClick={() => goTo("apps://")}>
            {t("apps.notFound.back")}
          </button>
        }
      />
    </section>
  );
}
