/**
 * 设置导航（渲染在**主侧栏**，PRD-07-057 两层左侧结构）。
 *
 * 设置此前自带一列分组导航，于是「活动栏 + 主侧栏 + 主区」的框架里出现了第二套导航列，
 * 而它左边那条主侧栏是空的——同一屏两种导航语言。现在设置与笔记/录音等目的地同构：
 * 侧栏 = 该目的地的列表（一层平铺的页面清单），主区 = 选中页的内容。
 *
 * **侧栏只有图标 + 名字，没有任何计数或状态点**（PRD-09-070 ⑤）：设置改完即时落盘，
 * 不存在"待保存"状态需要提示；而"与出厂默认不同"既不是导航级信息，又会被**应用自己写入的值**
 * （引导里选的 Vault、装模型时写的权重路径、引导里选的主题）点亮，读作纯噪声——用户会问
 * "为什么这几组有点、那几组没有"，而任何需要解释的标记都不该留在扫读路径上。
 * 真正要用的是行内那颗「恢复默认」（仅当该项被改过时出现，自带 tooltip）。
 *
 * 搜索态与常态是同一份清单，只是按命中数过滤并标出各页命中几条：一层平铺之后「页」
 * 就是最细的导航粒度，不再有"大类太粗、分组才是答案"这回事（那正是两层骨架的症状）。
 */

import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowClockwiseIcon,
  CpuIcon,
  DiscIcon,
  GlossaryIcon,
  ImageIcon,
  KeyboardIcon,
  LanguagesIcon,
  MicIcon,
  NotebookIcon,
  SettingsIcon,
  ShieldIcon,
  SparklesIcon,
  UserIcon,
} from "@lunarca/editor-core/Icon";
import { QueryBox } from "../../components/ui/QueryBox";
import { useResetSettingsPage } from "./use-reset-settings-page";
import { searchPages, searchSpecs } from "./settings-filter";
import { useSettingSearchText } from "./use-setting-search-text";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu } from "../../lib/context-menu";
import type { SettingsPageId } from "./settings-information-architecture";
import type { SettingsController } from "./use-settings-controller";
import { goTo } from "../../lib/navigate";

/**
 * 页面图标：让侧栏可扫读（纯图标不足以表意，故图标 + 文字并列）。
 *
 * **类型收紧为 `Record<SettingsPageId, …>` 且不留兜底**：漏登记一页就是编译错误，
 * 撞图标由 IA 守护测试拦住——「外部工具」与「高级」曾同为 `CpuIcon`，真机上看不出区别，
 * 而 typecheck / lint / 测试一个都不会红。
 */
export const PAGE_ICONS: Record<SettingsPageId, (props: { size?: number }) => ReactNode> = {
  account: UserIcon,
  general: SettingsIcon,
  appearance: ImageIcon,
  shortcuts: KeyboardIcon,
  ai_services: SparklesIcon,
  local_models: CpuIcon,
  voice_input: MicIcon,
  recording: DiscIcon,
  translate: LanguagesIcon,
  notes: NotebookIcon,
  glossary: GlossaryIcon,
  privacy: ShieldIcon,
};

/** 搜索态点某页 = 收起搜索、深链过去（全应用唯一的导航语义）。 */
function navigateToPage(page: SettingsPageId) {
  goTo(`settings://${page}`);
}

export function SettingsNav({ controller }: { controller: SettingsController }) {
  const { t } = useTranslation("settings");
  const contextMenu = useContextMenu();
  const resetPage = useResetSettingsPage(controller);
  const searching = controller.query.trim().length > 0;
  const searchText = useSettingSearchText();
  const matches = searchSpecs(controller.specs, controller.query, searchText);
  // 侧栏列出"这次搜索与它有关"的页：有行命中的，加上页名/页同义词自身命中的
  // （面板型页注册表零键，只按行命中过滤会让它们在搜索态从侧栏消失）。
  const pageHits = searchPages(controller.pages, controller.query, searchText);
  const matchingPages = searching
    ? controller.pages.filter(
        (page) => matches.some((spec) => spec.group === page) || pageHits.includes(page),
      )
    : [];

  /**
   * 右键「恢复默认」（整页，仅当确有改动）：破坏性且不可撤销，故照旧要一次确认——
   * 与页头按钮共用 Rust 变更预览和确认卡。
   * 文案与命令范围同屏一致：标准模式「恢复本组常用设置」→ `standard`，高级模式
   * 「恢复本组全部设置」→ `all`（TDD-07 §4.1b）。
   */
  function openResetMenu(event: ReactMouseEvent, page: SettingsPageId, modified: number) {
    const labelKey = controller.advancedMode ? "action.reset_group_all" : "action.reset_group_standard";
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "resetGroup",
          label: t(labelKey),
          icon: <ArrowClockwiseIcon size={13} />,
          danger: true,
          disabled: modified === 0,
          onSelect: () => {
            void resetPage(page);
          },
        },
      ]),
    );
  }

  return (
    <div className="luna-settings-nav">
      <QueryBox
        value={controller.query}
        onChange={controller.setQuery}
        placeholder={t("search.placeholder")}
        ariaLabel={t("search.placeholder")}
      />
      <label className="luna-settings-nav__filter">
        <input type="checkbox" checked={controller.modifiedOnly}
          onChange={(event) => controller.setModifiedOnly(event.target.checked)} />
        {t("search.modified_only")}
      </label>

      {searching && matchingPages.length === 0 ? (
        <p className="luna-settings-nav__empty">
          {t("search.empty", { query: controller.query.trim() })}
        </p>
      ) : null}

      <nav className="luna-settings-nav__list" aria-label={t("nav")}>
        {(searching ? matchingPages : controller.pages).map((page) => {
          const Icon = PAGE_ICONS[page];
          return (
            <button
              key={page}
              type="button"
              className="luna-settings-nav__item"
              data-active={!searching && controller.activePage === page}
              onClick={() => {
                controller.setModifiedOnly(false);
                if (searching) navigateToPage(page); else controller.selectPage(page);
              }}
              onContextMenu={(event) =>
                openResetMenu(event, page, controller.modifiedByPage[page] ?? 0)
              }
            >
              <Icon size={15} />
              <span className="luna-settings-nav__label">{t(`group.${page}`)}</span>
            </button>
          );
        })}
      </nav>

    </div>
  );
}
