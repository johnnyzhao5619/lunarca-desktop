/// <reference types="vite/client" />
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { TagTreeRow } from "./TagsView";
import type { TagTreeNode } from "./tag-tree";
import workspaceCss from "./workspace.css?raw";

const node: TagTreeNode = {
  name: "交易",
  path: "交易",
  directCount: 8,
  noteCount: 8,
  aiOnly: true,
  children: [],
};

function renderRow(selectedTag: string | null) {
  return renderToStaticMarkup(
    <TagTreeRow
      node={node}
      selectedTag={selectedTag}
      expanded={new Set()}
      onToggleExpand={vi.fn()}
      onSelectTag={vi.fn()}
      onContextMenu={vi.fn()}
      expandLabel="展开"
      collapseLabel="折叠"
      aiOnlyLabel="仅由 AI 标注"
      aiBadgeLabel="AI"
      showAiBadge={true}
    />,
  );
}

describe("TagTreeRow", () => {
  it("用 aria-current 作为唯一选中状态事实源", () => {
    const html = renderRow("交易");

    expect(html).not.toContain("data-selected");
    expect(html).toContain('aria-current="true"');
  });

  it("让完整树行消费正文按钮的 aria-current 状态", () => {
    expect(workspaceCss).toContain(
      ".luna-tags-tree__row:has(> .luna-tags-list__item[aria-current])",
    );
  });

  it("未选中行不暴露选中状态", () => {
    const html = renderRow("趋势分析");

    expect(html).not.toContain("data-selected");
    expect(html).not.toContain("aria-current");
  });
});
