import { describe, expect, it, vi } from "vitest";
import { noteMenuItems, type NoteMenuOptions } from "./note-menu";
import type { ContextMenuAction, ContextMenuItem } from "../../lib/context-menu";
import type { RecipeDto } from "../../ipc/bindings";

/** 桩 t：直接回显 key，断言里就能看出取的是哪条文案。 */
const t = (key: string) => key;

function ids(items: ContextMenuItem[]): string[] {
  return items.map((item) => item.id);
}

/** 按 id 取可执行项（分隔线没有禁用态，断言前先收窄类型）。 */
function actionById(items: ContextMenuItem[], id: string): ContextMenuAction {
  const found = items.find((item) => item.id === id);
  if (found?.kind !== "action") {
    throw new Error(`菜单里没有可执行项 ${id}`);
  }
  return found;
}

/** 注入的确认函数：默认"用户点了取消"，用例可覆盖。 */
let askResult = false;
const ask = vi.fn(async () => askResult);
/** 注入的输入卡：默认"用户取消了"（重命名的默认呈现面，见 `PromptHost`）。 */
const prompt = vi.fn(async () => null);

function menu(overrides: Partial<NoteMenuOptions> = {}, recipes: readonly RecipeDto[] = []) {
  return noteMenuItems(
    { path: "Notes/a.md", onStatus: () => {}, ...overrides },
    t,
    ["en", "ja"],
    ask,
    prompt,
    "可从回收站还原。",
    recipes,
  );
}

const recipe = (id: string, input: RecipeDto["input"]): RecipeDto => ({
  pack: "app.lunarca.creator-starter",
  pack_name: "创作者起步包",
  pack_version: "1.1.0",
  id,
  title: id,
  input,
  steps: [{ type: "vault_create_note", dir: "Projects" }],
  capabilities: { uses_ai: false, transcribes: false, translates: false, creates_notes_in: ["Projects"], exports: false },
});

describe("noteMenuItems", () => {
  it("最小形态：给对象**恒有**的那几个动作，不凭空长出打开方式与删除", () => {
    // 重命名恒在（单选）：菜单里的每一个对象都是库里的一个文件或文件夹，改名对它们
    // 一律成立。它此前要调用方显式传一个回调才出现，于是录音/转写/速记/搜索结果里
    // 统统没有这一项（2026-09-02 真机上报）。
    expect(ids(menu())).toEqual(["rename", "copyPath", "copyLunarcaLink", "reveal", "exportNote"]);
  });

  it("多选时只留对整批有定义的动作（打开方式 / 重命名 / 定位文件都不给）", () => {
    const items = ids(
      menu({
        paths: ["Notes/a.md", "Notes/b.md"],
        openInNotes: () => {},
        transcript: { busy: false },
        distillable: true,
        remove: { confirm: { title: "删除？" } },
      }),
    );
    expect(items).not.toContain("openInNotes");
    expect(items).not.toContain("rename");
    expect(items).not.toContain("reveal");
    // 队列动作按篇入队，多选没有定义（批量入口是统一处理队列本身）。
    expect(items).not.toContain("retranscribe");
    expect(items).not.toContain("diarize");
    // 对整批成立的都在：整批整理、整批导出、复制路径、删除。
    expect(items).toEqual([
      "distill",
      "exportTranscript",
      "transcript",
      "copyPath",
      "exportText",
      "danger",
      "delete",
    ]);
  });

  it("「汇总成一篇」只在多选且整批可汇总时出现（一条不是汇总，PRD-06-047）", () => {
    expect(ids(menu({ mergeable: true }))).not.toContain("merge");
    expect(ids(menu({ paths: ["a.md", "b.md"], mergeable: true }))).toContain("merge");
    expect(ids(menu({ paths: ["a.md", "b.md"], mergeable: false }))).not.toContain("merge");
  });

  it("打开方式随能力出现（缺哪个回调就没有哪一项）", () => {
    const items = menu({
      openInNotes: () => {},
      openInWindow: () => {},
    });
    expect(ids(items)).toEqual([
      "openInNotes",
      "openInWindow",
      "open",
      "rename",
      "copyPath",
      "copyLunarcaLink",
      "reveal",
      "exportNote",
    ]);
  });

  it("文件夹不给「导出为…」（没有正文可导）", () => {
    expect(ids(menu({ exportText: false }))).toEqual(["rename", "copyPath", "reveal"]);
  });

  it("单篇「导出为…」给齐四种落点：文本 / HTML / PDF / 笔记与附件（PRD-11-010/011）", () => {
    const item = menu().find((entry) => entry.kind === "submenu" && entry.id === "exportNote");
    expect(item && item.kind === "submenu" ? item.items.map((sub) => sub.id) : []).toEqual([
      "exportText",
      "exportHtml",
      "exportPdf",
      "exportBundle",
    ]);
  });

  it("文件树把重命名的输入面留在行内，别处走共享输入卡", () => {
    const inline = vi.fn();
    actionById(menu({ rename: { inline } }), "rename").onSelect();
    expect(inline).toHaveBeenCalledTimes(1);
    expect(prompt).not.toHaveBeenCalled();
    // 不给 `inline` 的视图：同一项改由那张唯一的输入卡承担。
    prompt.mockClear();
    actionById(menu(), "rename").onSelect();
    expect(prompt).toHaveBeenCalledTimes(1);
  });

  it("转写笔记才有队列动作，且翻译子菜单来自注册表语言清单", () => {
    const items = menu({ transcript: { busy: false } });
    expect(ids(items)).toContain("retranscribe");
    expect(ids(items)).toContain("diarize");
    const translate = items.find((item) => item.id === "translate");
    expect(translate?.kind).toBe("submenu");
    expect(translate?.kind === "submenu" && ids(translate.items)).toEqual([
      "translate-en",
      "translate-ja",
    ]);
  });

  it("「生成知识笔记」由 `distillable` 单独决定，且与转写能力各自独立（ADR-024）", () => {
    // 对话笔记没有转写队列动作却同样可蒸馏——两个开关若合并，对话行会长出
    // 一排点不动的转写项。
    expect(ids(menu({ distillable: true }))).toContain("distill");
    expect(ids(menu({ distillable: true }))).not.toContain("retranscribe");
    expect(ids(menu({ transcript: { busy: false } }))).not.toContain("distill");
    expect(ids(menu({}))).not.toContain("distill");
  });

  it("「沉淀为知识」只对记录态出现（PRD-06-053）", () => {
    // 判据是 Rust 下发的已判定布尔，不是"路径像草稿"这类前端推断（ADR-003）。
    expect(ids(menu({ settleable: true }))).toContain("settle");
    expect(ids(menu({ settleable: false }))).not.toContain("settle");
    expect(ids(menu({}))).not.toContain("settle");
  });

  it("记录态同时给一键沉淀与 AI 辅助沉淀，两项互不取代（PRD-06-054）", () => {
    // 这是 PRD-06-054 对批次 5 的修订：此前断言的是"一篇最多落其中一档"。
    // 现在记录态两个动作**都给**——一个原地去掉 `luna.stage` 一个键，一个跑模型另出
    // 一篇知识笔记且**源留在原地不改 stage**，于是它们既不互斥也不互相取消。
    const both = ids(menu({ settleable: true, distillable: true }));
    expect(both).toContain("settle");
    expect(both).toContain("distill");
    // 两个开关仍各自独立：资料只有蒸馏（没有 stage 可去），谁都不隐式带出另一个。
    expect(ids(menu({ distillable: true }))).not.toContain("settle");
    expect(ids(menu({ settleable: true }))).not.toContain("distill");
  });

  it("「升级为笔记」只对速记出现，与蒸馏各自独立（PRD-02-055）", () => {
    // 速记走收件箱闭环（升级），资料走蒸馏（生成纪要）——两条线不该串。
    expect(ids(menu({ promotable: true }))).toContain("promote");
    expect(ids(menu({ promotable: true }))).not.toContain("distill");
    expect(ids(menu({ distillable: true }))).not.toContain("promote");
    expect(ids(menu({}))).not.toContain("promote");
  });

  it("「钉到桌面」只对速记出现，且按此刻的状态给出两种说法（ADR-119）", () => {
    // 给一个恒为「钉到桌面」的菜单项，用户点第二次会以为自己开了第二张便签。
    const unpinned = actionById(menu({ pinnedToDesktop: false }), "desktop-note");
    const pinned = actionById(menu({ pinnedToDesktop: true }), "desktop-note");
    expect(unpinned.label).not.toBe(pinned.label);
    // 不是速记就整条不出现——「贴到桌面上」对一段录音或一篇主题页没有意义。
    expect(ids(menu({}))).not.toContain("desktop-note");
  });

  it("该笔记已有转写类任务在跑时，入队动作置灰（与回放视图互斥口径一致）", () => {
    const items = menu({ transcript: { busy: true } });
    expect(actionById(items, "retranscribe").disabled).toBe(true);
    expect(actionById(items, "diarize").disabled).toBe(true);
    const translate = items.find((item) => item.id === "translate");
    expect(translate?.kind === "submenu" && translate.disabled).toBe(true);
    // 导出不依赖队列，不该被顺手关掉。
    const exportItem = items.find((item) => item.id === "exportTranscript");
    expect(exportItem?.kind === "submenu" && exportItem.disabled).toBeUndefined();
  });

  it("删除项标为破坏性，且未确认前不发命令", async () => {
    // 确认走共享宿主（`ConfirmFn`）而不再是原生 `window.confirm`：此处注入一个
    // "用户点了取消"的实现，验证什么都不会发生。
    askResult = false;
    ask.mockClear();
    const onDeleted = vi.fn();
    const confirmOptions = { title: "删除这篇笔记？", description: "可从回收站还原。" };
    const items = menu({ remove: { confirm: confirmOptions, onDeleted } });
    const del = items.find((item) => item.id === "delete");
    expect(del?.kind === "action" && del.danger).toBe(true);
    if (del?.kind === "action") {
      del.onSelect();
    }
    await vi.waitFor(() => expect(ask).toHaveBeenCalledWith(confirmOptions));
    expect(onDeleted).not.toHaveBeenCalled();
  });

  it("「运行工作流」：接在录音上的只给录音 / 转写笔记，其余给有正文的笔记；没有可运行的就没有这一项（纵切 7b B5）", () => {
    const recipes = [recipe("from-recording", "recording"), recipe("from-note", "note")];
    const titles = (items: ContextMenuItem[]) => {
      const found = items.find((item) => item.id === "recipes");
      return found?.kind === "submenu" ? found.items.map((item) => item.id) : null;
    };
    expect(titles(menu({}, recipes))).toEqual(["recipe-app.lunarca.creator-starter-from-note"]);
    expect(titles(menu({ transcript: { busy: false } }, recipes))).toEqual([
      "recipe-app.lunarca.creator-starter-from-recording",
      "recipe-app.lunarca.creator-starter-from-note",
    ]);
    expect(titles(menu({ exportText: false }, recipes))).toBeNull();
    expect(titles(menu({ paths: ["Notes/a.md", "Notes/b.md"] }, recipes))).toBeNull();
    expect(titles(menu())).toBeNull();
  });

  it("运行前先确认；取消就什么都不发", async () => {
    askResult = false;
    ask.mockClear();
    const items = menu({}, [recipe("from-note", "note")]);
    const found = items.find((item) => item.id === "recipes");
    if (found?.kind !== "submenu") throw new Error("没有「运行工作流」");
    const run = found.items[0];
    if (run?.kind !== "action") throw new Error("不是可执行项");
    run.onSelect();
    await vi.waitFor(() => expect(ask).toHaveBeenCalledTimes(1));
  });

  it("视图特有项置顶，并与通用项之间自带分隔（不留裸分隔线）", () => {
    const items = menu({
      leading: [{ kind: "action", id: "focus", label: "focus", onSelect: () => {} }],
    });
    expect(ids(items).slice(0, 3)).toEqual(["focus", "leading", "rename"]);
    expect(items[1].kind).toBe("separator");
  });
});
