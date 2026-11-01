/**
 * 主侧栏顶部的二级分区切换（PRD-07-057 ②）。
 *
 * 二级选择**只改变该空间的查询投影**，不制造第二份内容——同一篇笔记在「速记」与
 * 「全部笔记」里是同一个文件（PRD-07-006）。
 *
 * **无分区的空间不渲染它**（只有知识库有分区，PRD-07-002）：其余空间的侧栏内容是视图
 * 内部结构，摆一条只有一格的段控等于告诉用户"这里还有别的分区"——那是伪分区。
 *
 * 分区项可挂角标（形态与一级角标同源）；当前没有任何分区需要它——速记待处理数挂在
 * 「速记」一级图标上（PRD-07-066）。
 */

import { useTranslation } from "react-i18next";
import {
  navBadgeToRender,
  sectionLabelKey,
  sectionsOf,
  type NavBadge,
  type WorkspaceSection,
  type WorkspaceSpace,
} from "./workspace-ia";

export interface WorkspaceSectionNavProps {
  space: WorkspaceSpace;
  activeSection: WorkspaceSection | null;
  badges: Partial<Record<WorkspaceSection, NavBadge>>;
  onSelect: (section: WorkspaceSection) => void;
}

export function WorkspaceSectionNav({
  space,
  activeSection,
  badges,
  onSelect,
}: WorkspaceSectionNavProps) {
  const { t } = useTranslation("workspace");
  const sections = sectionsOf(space);
  if (sections.length === 0) {
    return null;
  }
  return (
    <nav className="luna-workspace__sections" aria-label={t("nav.sectionNav")}>
      {sections.map((section) => {
        const badge = navBadgeToRender(badges[section.id]);
        const name = t(sectionLabelKey(space, section.id));
        return (
          <button
            key={section.id}
            type="button"
            className="luna-workspace__section-btn"
            // 取值是 `true` 而不是 `page`：一级图标已经宣告了「当前页」，二级说的是
            // 「这一组里当前是哪一项」——两层都写 `page` 会让读屏在同一屏念两次当前页。
            // 属性**在不在**才是样式判据（`[aria-current]`），故两处仍是同一条口径（裁决 D5）。
            aria-current={section.id === activeSection ? "true" : undefined}
            onClick={() => onSelect(section.id)}
          >
            <span className="luna-workspace__section-label">{name}</span>
            {badge ? (
              <span
                className="luna-workspace__section-badge"
                aria-label={t(badge.labelKey, { label: name, count: badge.count })}
              >
                {badge.count}
              </span>
            ) : null}
          </button>
        );
      })}
    </nav>
  );
}
