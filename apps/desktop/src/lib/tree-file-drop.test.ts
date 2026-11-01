// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { TREE_FILE_DROP_EVENT, dropOnTree, markTreeDropTarget, treeDropAt, type TreeFileDrop } from "./tree-file-drop";

/** 一棵最小的文件树：根 section（落点 ""）+ 文件夹行「项目」+ 其中一篇笔记的行。 */
function mountTree() {
  document.body.innerHTML = `
    <section class="luna-note-tree" data-drop-id="">
      <button data-dir="true" data-drop-id="项目" id="folder">项目</button>
      <button data-dir="false" data-drop-id="项目" id="note">计划</button>
      <div id="blank">空白</div>
    </section>
    <main id="elsewhere">编辑区</main>
  `;
  const byId = (id: string) => document.getElementById(id) as HTMLElement;
  return { tree: document.querySelector<HTMLElement>(".luna-note-tree")!, byId };
}

function pointAt(element: Element | null) {
  document.elementFromPoint = vi.fn(() => element);
}

const original = document.elementFromPoint;
afterEach(() => {
  document.elementFromPoint = original;
  document.body.innerHTML = "";
});

describe("tree-file-drop", () => {
  it("a folder row is its own target; a note row and blank space fall back to their folder", () => {
    const { byId } = mountTree();
    pointAt(byId("folder"));
    expect(treeDropAt(0, 0)?.folder).toBe("项目");
    pointAt(byId("note"));
    expect(treeDropAt(0, 0)?.folder).toBe("项目");
    pointAt(byId("blank"));
    expect(treeDropAt(0, 0)?.folder).toBe("");
    pointAt(byId("elsewhere"));
    expect(treeDropAt(0, 0)).toBeNull();
  });

  it("highlights the folder row (not the note row) and clears it afterwards", () => {
    const { tree, byId } = mountTree();
    pointAt(byId("note"));
    markTreeDropTarget(treeDropAt(0, 0));
    expect(byId("folder").dataset.fileDrop).toBe("true");
    expect(byId("note").dataset.fileDrop).toBeUndefined();
    pointAt(byId("blank"));
    markTreeDropTarget(treeDropAt(0, 0));
    expect(tree.dataset.fileDrop).toBe("true");
    expect(byId("folder").dataset.fileDrop).toBeUndefined();
    markTreeDropTarget(null);
    expect(tree.dataset.fileDrop).toBeUndefined();
  });

  it("hands the drop to the tree, and declines drops outside it", () => {
    const { tree, byId } = mountTree();
    const received: TreeFileDrop[] = [];
    tree.addEventListener(TREE_FILE_DROP_EVENT, (event) => received.push((event as CustomEvent<TreeFileDrop>).detail));
    pointAt(byId("note"));
    expect(dropOnTree(["/Users/me/Desktop/计划.md"], 0, 0)).toBe(true);
    expect(received).toEqual([{ paths: ["/Users/me/Desktop/计划.md"], folder: "项目" }]);
    pointAt(byId("elsewhere"));
    expect(dropOnTree(["/x.md"], 0, 0)).toBe(false);
    expect(received).toHaveLength(1);
  });
});
