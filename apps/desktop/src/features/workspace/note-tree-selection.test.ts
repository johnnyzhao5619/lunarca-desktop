/// <reference types="vite/client" />
/**
 * 文件树与清单的**改名与多选**守护（PRD-06-006 / PRD-06-059 / PRD-07-059）。
 *
 * 三个真机上报的缺陷各对应一组用例（2026-09-02）：
 * ① 行内改名每敲一个字符就把整串重新全选，改不完一个名字；
 * ② 文件不能多选，于是删五个要点五次；
 * ③ 录音清单右键里根本没有「重命名」——因为改名被实现成了"文件树的行内输入态"，
 *    而不是对象的能力。
 *
 * 仓库没有为这两个组件配 jsdom 渲染夹具（同 `note-tree-real-tree.test.ts` 的先例），故按
 * 源码断言；这里拦的恰好都是源码层面的复发。①的**行为**另有真渲染守护
 * （`components/prompt-host.test.tsx`），两者互补：那条钉"用户打出来的字对不对"，
 * 这条钉"依赖有没有又把草稿放回去"。
 */

import { describe, expect, it } from "vitest";

const sources = import.meta.glob("./{NoteTree.tsx,FilteredNoteList.tsx,note-menu.tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const TREE = sources["./NoteTree.tsx"] ?? "";
const LIST = sources["./FilteredNoteList.tsx"] ?? "";
const MENU = sources["./note-menu.tsx"] ?? "";

/** 选择原语本体：行与容器的接线、以及选中态属性都由它一处贴出。 */
const HOOK = Object.values(
  import.meta.glob("./use-note-selection.ts", {
    eager: true,
    import: "default",
    query: "?raw",
  }) as Record<string, string>,
)[0] as unknown as string;

/** 只看会执行的代码：注释里解释「它曾经是什么样」是说明，不是残留。 */
function code(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");
}

describe("行内改名（PRD-06-006）", () => {
  it("扫描面非空", () => {
    expect(TREE.length).toBeGreaterThan(1000);
    expect(LIST.length).toBeGreaterThan(1000);
    expect(MENU.length).toBeGreaterThan(1000);
  });

  it("「聚焦并全选」依赖的是**这是哪一次编辑**，草稿不在其中", () => {
    // 草稿一旦回到依赖里，每敲一个字符就会重新 `select()`，下一个字符直接覆盖全部内容。
    expect(code(TREE)).toContain("}, [editingKey]);");
    expect(code(TREE)).not.toContain("}, [editing]);");
    // 编辑态的身份里不含草稿（含了就一定会被写进依赖）。
    const editingState = code(TREE).slice(
      code(TREE).indexOf("interface EditingState"),
      code(TREE).indexOf("export interface NoteTreeProps"),
    );
    expect(editingState).not.toContain("draft: string");
  });

  it("新建与改名共用**同一个**输入框实现", () => {
    // 两份逐字重复的 `<input>` 正是"缺陷只在其中一份里被看见、修的时候只修一处"的土壤。
    expect(code(TREE).match(/<input/gu) ?? []).toHaveLength(1);
    expect(code(TREE)).toContain("function renderEditInput(");
  });

  it("改名走唯一那条命令，前端不拼路径、不补后缀", () => {
    expect(code(TREE)).toContain("commands.vaultRename(current.path, name)");
    expect(code(TREE)).not.toContain("commands.renameNote");
    // 目录与笔记**不再分两条分支**：拼 `父目录/新名` 与补 `.md` 都归 Rust（ADR-003），
    // 前端这一段路径拼接曾在文件树、清单文件夹改名与「移动到」三处各写了一遍。
    expect(code(TREE)).not.toContain("const parent = parentPathOf(current.path)");
    expect(code(TREE)).not.toContain("if (current.isDir) {");
    // 改名的草稿 = 行上看到的那个名字，与行显示同一个函数（不在这里再判一次后缀）。
    expect(code(TREE)).toContain("setEditDraft(noteStem(path))");
    // 拒绝原因回流而不是泛泛一句「操作失败」：Rust 已经算好了"名称不能包含路径分隔符"这类
    // 解释。它发的是「码: 载荷」（ADR-105），故经 `vaultErrorText` 换成句子再上屏。
    expect(code(TREE)).toContain("onStatus(vaultErrorText(result.error))");
  });
});

describe("多选（PRD-06-059）", () => {
  it("笔记树与清单共用同一套选择原语，不各写一份", () => {
    for (const source of [TREE, LIST]) {
      expect(code(source)).toContain("use-note-selection");
      expect(code(source)).toContain("selection.click(");
      expect(code(source)).toContain("selection.contextClick(");
      expect(code(source)).toContain("selection.contains(");
    }
  });

  it("加选/连选**不打开**（否则每加选一条工作面就被切走一次）", () => {
    // 判据：打开动作恒在 `selection.click(...)` 返回真时才发生。
    expect(code(TREE)).toContain("if (!selection.click(item.path, event)) {");
    expect(code(LIST)).toContain("if (selection.click(item.path, event)) {");
  });

  it("键盘契约整条住在原语里：行与容器都从它拿接线", () => {
    // macOS 的 WKWebView 点击 `<button>` 不移动焦点、且 mousedown 的默认行为把焦点收回
    // body，于是键盘处理器（挂在容器上）永远收不到按键（2026-09-02 真机：↑↓ 变成滚动
    // 页面、空格翻页、Esc/Delete 无反应）。抢焦点这件事只该有一份实现，故行与容器一律
    // 从 `selection` 上取接线，不各写一遍。
    for (const source of [TREE, LIST]) {
      expect(code(source)).toContain("selection.rowProps(item.path)");
      expect(code(source)).toContain("selection.containerProps");
    }
    const hook = HOOK;
    expect(code(hook)).toContain("event.preventDefault()");
    expect(code(hook)).toContain("focus({ preventScroll: true })");
    // 连程序性聚焦都被拒时退一步聚焦容器——键盘处理器就挂在那儿。
    expect(code(hook)).toContain("document.activeElement !== row");
    // Delete 键：Mac 键盘上标着 delete 的那个键在浏览器里叫 `Backspace`，只认 `Delete`
    // 的实现在 Mac 上等于没做。
    expect(code(hook)).toContain('event.key === "Delete" || event.key === "Backspace"');
  });

  it("文件树也给「移动到 ▸」，候选是整棵真实的树且不含非法落点", () => {
    // 拖拽是近距离直接操作，菜单是远距离与键盘那条路——同一件事的两条路，不是两套说法。
    expect(code(TREE)).toContain("dirChoices(rendered");
    expect(code(TREE)).toContain("onMove: (targetDir, paths) => void moveEntries(paths, targetDir)");
    // 选中的文件夹自己与它的子孙不该出现在候选里（那是 Rust 会整批拒绝的落点）。
    expect(code(TREE)).toContain("choice.path !== path && !choice.path.startsWith(`${path}/`)");
  });

  it("选中态用 aria-selected，当前打开的那条仍是 aria-current（裁决 D5）", () => {
    // `aria-selected` 由原语随 `rowProps` 一起贴，视图不各写一遍——漏写不报错，只是那一格
    // 永远不高亮（真机 2026-09-09 速记就是这么坏的）。视图只负责"哪一条正开着"。
    expect(code(HOOK)).toContain('"aria-selected": selection.paths.has(path)');
    for (const source of [TREE, LIST]) {
      expect(code(source)).toContain("{...selection.rowProps(item.path)}");
      expect(code(source)).toContain("aria-current={selectedPath === item.path || undefined}");
    }
    // 声明了角色就要兑现键盘：方向键 / 连选 / Esc / Delete 由选择原语在容器上接住。
    expect(code(TREE)).toContain("{...selection.containerProps}");
    expect(code(LIST)).toContain("...selection.containerProps,");
  });

  it("批量动作作用于**整批**：拖拽、删除、右键各自都不退回单条", () => {
    // 拖起选中集里的任一行 = 整批一起走。
    expect(code(TREE)).toContain("selection.contains(item.path) ? selection.ordered : [item.path]");
    expect(code(TREE)).toContain("usePointerDrag<readonly string[]>");
    // 顶栏删除按钮作用于选择集，且多选时按钮自己会报数。
    expect(code(TREE)).toContain("void deleteSelection()");
    expect(code(TREE)).toContain('tCommon("menu.deleteSelected", { count: selection.ordered.length })');
    // 一次确认、一条命令（剪枝、幂等与逐项成败归 Rust）。
    expect(code(TREE)).toContain("deleteNotesWithConfirm({");
    expect(code(MENU)).toContain("commands.vaultDelete([...options.paths], null)");
  });

  it("右键的作用对象**同一拍**算好，不去读一个还没更新的选择集", () => {
    // `contextClick` 排的是一次 React 状态更新，而菜单在同一拍里就要装配好条目——
    // 右键一行选中集之外的笔记时，此刻读到的 `ordered` 还是上一批，菜单会作用在
    // 用户没有右键的对象上。
    expect(code(TREE)).toContain("selection.targetsFor(node.path)");
    expect(code(LIST)).toContain("selection.targetsFor(item.path)");
    for (const source of [TREE, LIST]) {
      expect(code(source)).not.toMatch(/contextClick\([^)]*\);\n\s*const targets = selection\.ordered/u);
    }
  });

  it("批量命令的成败**逐项落地**：`ok` 不等于每一项都做成了", () => {
    // 2026-09-02 真机：拖四个成功三个，界面只字未提——用户重启之后自己对着文件夹数才
    // 发现。命令返回的 `moved` / `deleted` 是**真正做成的那些**，`failed` 逐项带理由。
    expect(code(TREE)).toContain("result.data.moved");
    expect(code(TREE)).toContain("result.data.failed.length > 0");
    expect(code(TREE)).toContain('tCommon("menu.batchFailed"');
    // 收尾按**真正删掉的那些**走：拿提交的那一批收尾会关掉一篇其实还在的笔记。
    expect(code(TREE)).toContain("onDeleted: (deleted) => deleted.forEach(afterDeleted)");
    expect(code(MENU)).toContain("const { deleted, failed } = result.data;");
  });

  it("两项 AI 动作的判据都是 Rust 逐行下发的位，前端一位都不拼", () => {
    // 「生成知识笔记」= `distillable`（资料 ∪ 记录）；「汇总成一篇」= `can_merge`
    // （只挡聚合页，PRD-06-047 2026-09-02 修订）。用 `draft` 顶替任何一个，都会把资料
    // 挡在门外——那正是真机上"批量生成知识笔记/汇总没有入口"的由来。
    expect(code(TREE)).toContain("findTreeNode(rendered, p)?.distillable === true");
    expect(code(TREE)).toContain("findTreeNode(rendered, p)?.can_merge === true");
    expect(code(TREE)).not.toContain("?.draft === true");
  });

  it("多选菜单只留对整批有定义的动作，且不是另一份菜单", () => {
    // 同一个构建器 + 一个 `batch` 开关；另写一份的表现是同一批笔记在两个视图里能做的
    // 事悄悄分叉（本文件存在的理由）。
    expect(code(MENU)).toContain("const batch = count > 1;");
    expect(code(MENU)).toContain("!batch && options.openInNotes");
    expect(code(MENU)).toContain("!batch && options.transcript");
    expect(code(MENU)).toMatch(/!batch && \{[\s\S]{0,80}id: "rename"/u);
  });
});

describe("重命名是对象的能力（PRD-07-059）", () => {
  it("菜单恒给重命名，行内输入态只是文件树的呈现优势", () => {
    // 它此前是个要调用方显式传的回调，于是录音/转写/速记/搜索结果里统统没有这一项。
    expect(code(MENU)).toContain("options.rename?.inline ??");
    expect(code(MENU)).toContain("renameEntryWithPrompt({");
    expect(code(TREE)).toContain("rename: { inline: () => startRename(node.path) }");
  });

  it("改名的落盘只有一处：唯一那条 `vault_rename`", () => {
    expect(code(MENU)).toContain("commands.vaultRename(options.path, name)");
    // 文件夹改名（清单侧）也走它——此前那里自己算 `父目录/新名`。
    const folders = Object.values(
      import.meta.glob("./use-note-folders.ts", {
        eager: true,
        import: "default",
        query: "?raw",
      }) as Record<string, string>,
    )[0] as unknown as string;
    expect(code(folders)).toContain("commands.vaultRename(path, name)");
    expect(code(folders)).not.toContain("commands.vaultMove(");
  });
});
