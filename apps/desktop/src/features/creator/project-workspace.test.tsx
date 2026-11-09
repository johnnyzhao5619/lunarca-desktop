// @vitest-environment happy-dom
/**
 * 项目工作区（ADR-165 ⑥ / PRD-19-037）：页签、阶段步进条与「下一步」都按 Rust 给的那一份
 * （`creator_project`）渲染；编辑器由宿主给，项目里的笔记不离开创作空间。
 */
import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { ProjectStageDto, ProjectWorkspaceDto, WorkspaceMetricEntryResultDto } from "../../ipc/bindings";
import type { CreateActionContext } from "../capture/create-actions";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { PromptHost } from "../../components/PromptHost";
import { ProjectWorkspace, paneFor, type ProjectPane } from "./ProjectWorkspace";

const HOME = "Creator/P/P.md";
const ENTRY: WorkspaceMetricEntryResultDto = "arrived";
/** 修复动作要用的入口（录制页签的开录前检查）。 */
const CREATE_ACTIONS: CreateActionContext = {
  captureText: vi.fn(async () => ENTRY),
  captureVoice: vi.fn(async () => ENTRY),
  createNote: vi.fn(async () => ENTRY),
  startRecording: vi.fn(async () => ENTRY),
  importMaterial: vi.fn(async () => ENTRY),
};
const STAGES: ProjectStageDto[] = ["idea", "research", "writing", "recording", "editing", "publishing", "published"];

function project(overrides: Partial<ProjectWorkspaceDto> = {}): ProjectWorkspaceDto {
  return {
    home: HOME,
    title: "P",
    stage: "writing",
    stages: STAGES,
    research: "Creator/P/P-研究.md",
    script: "Creator/P/P-脚本.md",
    next_action: "open_teleprompter",
    channels: ["youtube", "bilibili"],
    publish_date: null,
    archived: false,
    stage_hint: null,
    ...overrides,
  };
}

interface Rendered {
  onPane: ReturnType<typeof vi.fn>;
  onActiveDocument: ReturnType<typeof vi.fn>;
  projectNav: Record<"onOpen" | "onRenamed" | "onDeleted" | "onChanged", ReturnType<typeof vi.fn>>;
}

function show(pane: ProjectPane = { tab: "overview" }, data: ProjectWorkspaceDto = project()): Rendered {
  setCommandReply("creatorProject", { status: "ok", data });
  setCommandReply("creatorProjectOverview", { status: "ok", data: { stage: data.stage, items: [], exports: [] } });
  setCommandReply("creatorProjectQuotes", { status: "ok", data: [] });
  setCommandReply("creatorSwipeLibrary", { status: "ok", data: { groups: [], total: 0 } });
  const onPane = vi.fn();
  const onActiveDocument = vi.fn();
  const projectNav = { onOpen: vi.fn(), onRenamed: vi.fn(), onDeleted: vi.fn(), onChanged: vi.fn() };
  render(
    withI18n(
      createElement(
        ConfirmHost,
        null,
        createElement(PromptHost, null, createElement(ContextMenuHost, null, createElement(ProjectWorkspace, {
          home: HOME,
          pane,
          onPane,
          onBoard: () => {},
          renderEditor: (path: string): ReactNode => createElement("div", { "data-testid": "editor" }, path),
          renderPublish: (): ReactNode => createElement("div", { "data-testid": "publish" }),
          onImportQueued: () => {},
          onOpenInNotes: () => {},
          onActiveDocument,
          scriptOutline: null,
          createActions: CREATE_ACTIONS,
          projectNav,
          onItemMenu: vi.fn(),
        }))),
      ),
    ),
  );
  return { onPane, onActiveDocument, projectNav };
}

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("项目工作区", () => {
  it("概览就是项目首页：主页笔记在这里编辑，旁边是项目里的东西；向宿主报告正在编辑哪一篇", async () => {
    const { onActiveDocument } = show();
    expect(await screen.findByRole("heading", { name: "P" })).toBeTruthy();
    expect(screen.getByTestId("editor").textContent).toBe(HOME);
    expect(screen.getByRole("heading", { name: t("workspace:creator.overview.title") })).toBeTruthy();
    expect(onActiveDocument).toHaveBeenCalledWith(HOME);
  });

  it("页签只列存在的那几篇：没有研究笔记就没有研究页签，没有脚本就没有脚本与录制", async () => {
    show({ tab: "overview" }, project({ research: null }));
    await screen.findByRole("tab", { name: t("workspace:creator.project.tab.script") });
    expect(screen.queryByRole("tab", { name: t("workspace:creator.project.tab.research") })).toBeNull();
    expect(screen.getByRole("tab", { name: t("workspace:creator.project.tab.recording") })).toBeTruthy();
    expect(screen.getByRole("tab", { name: t("workspace:creator.project.tab.publish") })).toBeTruthy();
    cleanup();
    show({ tab: "overview" }, project({ script: null }));
    await screen.findByRole("tab", { name: t("workspace:creator.project.tab.publish") });
    expect(screen.queryByRole("tab", { name: t("workspace:creator.project.tab.recording") })).toBeNull();
  });

  it("录制页签：缺什么写明并给去处；都就绪才能开始跟读；点重来标记回到脚本那一行", async () => {
    setCommandReply("creatorRecordingCheck", {
      microphone: { kind: "needs_attention", reason: "microphone_permission_required", repair: { kind: "request_microphone_permission" } },
      transcriber: { kind: "available" },
    });
    setCommandReply("creatorScriptRetakes", {
      status: "ok",
      data: [{ line: 6, text: "大家好。", note: "重来 ×2 · 20:41、20:43" }],
    });
    setCommandReply("teleprompterOpen", { status: "ok", data: null });
    setCommandReply("requestPermission", null);
    setCommandReply("listSettings", { status: "ok", data: [] });
    const reveals: unknown[] = [];
    const onReveal = (event: Event) => reveals.push((event as CustomEvent).detail);
    window.addEventListener("luna:editor-reveal", onReveal);
    const { onPane } = show({ tab: "recording" });
    expect(await screen.findByText(t("workspace:create.reason.micPermissionRequired"))).toBeTruthy();
    // 跟读默认关（ADR-176 ④）：主按钮是「开启跟读并开始」。
    const start = screen.getByRole("button", { name: t("workspace:creator.recording.enableFollow") }) as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:create.repair") }));
    expect(commandMock("requestPermission")).toHaveBeenCalledWith("microphone");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.recording.openOnly") }));
    expect(commandMock("teleprompterOpen")).toHaveBeenCalledWith("Creator/P/P-脚本.md", false);

    fireEvent.click(await screen.findByText("大家好。"));
    expect(onPane).toHaveBeenCalledWith({ tab: "script" });
    expect(reveals).toEqual([{ path: "Creator/P/P-脚本.md", line: 6, headingIndex: null, focusEditor: true }]);
    window.removeEventListener("luna:editor-reveal", onReveal);
  });

  it("录制页签：麦克风与转写都就绪时「开始跟读」打开提词器并直接开跟读", async () => {
    setCommandReply("creatorRecordingCheck", { microphone: { kind: "available" }, transcriber: { kind: "available" } });
    setCommandReply("creatorScriptRetakes", { status: "ok", data: [] });
    setCommandReply("teleprompterOpen", { status: "ok", data: null });
    setCommandReply("listSettings", {
      status: "ok",
      data: [{ key: "teleprompter.follow_enabled", current_value: { kind: "bool", value: true }, control: { kind: "toggle" } }],
    });
    show({ tab: "recording" });
    expect(await screen.findByText(t("workspace:creator.recording.noRetakes"))).toBeTruthy();
    const start = (await screen.findByRole("button", { name: t("workspace:creator.recording.startFollow") })) as HTMLButtonElement;
    await waitFor(() => expect(start.disabled).toBe(false));
    fireEvent.click(start);
    await waitFor(() => expect(commandMock("teleprompterOpen")).toHaveBeenCalledWith("Creator/P/P-脚本.md", true));
    expect(commandMock("setSetting")).not.toHaveBeenCalled();
  });

  it("录制页签：跟读关着（默认）时一键「开启跟读并开始」——先写开关再开跟读", async () => {
    setCommandReply("creatorRecordingCheck", { microphone: { kind: "available" }, transcriber: { kind: "available" } });
    setCommandReply("creatorScriptRetakes", { status: "ok", data: [] });
    setCommandReply("teleprompterOpen", { status: "ok", data: null });
    setCommandReply("listSettings", { status: "ok", data: [] });
    setCommandReply("setSetting", { status: "ok", data: null });
    show({ tab: "recording" });
    const start = (await screen.findByRole("button", { name: t("workspace:creator.recording.enableFollow") })) as HTMLButtonElement;
    await waitFor(() => expect(start.disabled).toBe(false));
    fireEvent.click(start);
    await waitFor(() => expect(commandMock("teleprompterOpen")).toHaveBeenCalledWith("Creator/P/P-脚本.md", true));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.follow_enabled", { kind: "bool", value: true });
  });

  it("脚本页签放脚本的编辑器；发布页签放发布包、没有编辑器", async () => {
    show({ tab: "script" });
    await waitFor(() => expect(screen.getByTestId("editor").textContent).toBe("Creator/P/P-脚本.md"));
    cleanup();
    const { onActiveDocument } = show({ tab: "publish" });
    expect(await screen.findByTestId("publish")).toBeTruthy();
    expect(screen.queryByTestId("editor")).toBeNull();
    expect(onActiveDocument).toHaveBeenLastCalledWith(null);
  });

  it("步进条标出当前阶段，点另一格就把项目移过去并重读", async () => {
    setCommandReply("creatorSetProjectStage", { status: "ok", data: { view: "board", stages: [], hidden_stages: [], projects: [], archived: [] } });
    show();
    const current = await screen.findByRole("button", { name: t("workspace:creator.stage.writing") });
    expect(current.getAttribute("aria-current")).toBe("step");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.stage.editing") }));
    await waitFor(() => expect(commandMock("creatorSetProjectStage")).toHaveBeenCalledWith(HOME, "editing"));
    await waitFor(() => expect(commandMock("creatorProject")).toHaveBeenCalledTimes(2));
  });

  it("「下一步」随阶段：撰稿打开提词器，剪辑与发布去发布页签，已发布去分发页签", async () => {
    setCommandReply("teleprompterOpen", { status: "ok", data: null });
    show();
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.nextAction.open_teleprompter") }));
    await waitFor(() => expect(commandMock("teleprompterOpen")).toHaveBeenCalledWith("Creator/P/P-脚本.md", false));
    cleanup();

    // 录制阶段：先到录制页签看开录前检查。
    const recording = show({ tab: "overview" }, project({ stage: "recording", next_action: "start_follow" }));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.nextAction.start_follow") }));
    expect(recording.onPane).toHaveBeenCalledWith({ tab: "recording" });
    cleanup();

    const { onPane } = show({ tab: "overview" }, project({ stage: "editing", next_action: "import_final_cut" }));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.nextAction.import_final_cut") }));
    expect(onPane).toHaveBeenCalledWith({ tab: "publish" });
    cleanup();

    // 已发布：去分发页签改写与挑短视频。
    const published = show({ tab: "overview" }, project({ stage: "published", next_action: "distribute" }));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.nextAction.distribute") }));
    expect(published.onPane).toHaveBeenCalledWith({ tab: "distribute" });
    cleanup();

    // 没有下一步时页头不放主按钮。
    show({ tab: "overview" }, project({ stage: "published", next_action: null }));
    const header = within((await screen.findByRole("heading", { name: "P" })).closest("header") as HTMLElement);
    for (const action of ["write_idea", "import_material", "open_teleprompter", "start_follow", "import_final_cut", "open_publish", "distribute"]) {
      expect(header.queryByRole("button", { name: t(`workspace:creator.nextAction.${action}`) })).toBeNull();
    }
  });

  it("计划发布日期：选一天写回项目主页并重读；有日期时可以清掉", async () => {
    setCommandReply("creatorSetPublishDate", { status: "ok", data: { view: "board", stages: [], hidden_stages: [], projects: [], archived: [] } });
    show();
    // 没排期时是一个按钮（空日期框在 WebKit 里会把今天淡淡地画出来，像已经排了）。
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.project.date.set") }));
    const input = screen.getByLabelText(t("workspace:creator.project.date.label")) as HTMLInputElement;
    expect(input.value).toBe("");
    fireEvent.change(input, { target: { value: "2026-10-24" } });
    await waitFor(() => expect(commandMock("creatorSetPublishDate")).toHaveBeenCalledWith(HOME, "2026-10-24"));
    await waitFor(() => expect(commandMock("creatorProject")).toHaveBeenCalledTimes(2));
    cleanup();
    show({ tab: "overview" }, project({ publish_date: "2026-10-24" }));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.project.date.clear") }));
    await waitFor(() => expect(commandMock("creatorSetPublishDate")).toHaveBeenLastCalledWith(HOME, null));
  });

  it("从项目里点开的其它笔记是一张可关的卡片，关掉回概览", async () => {
    const { onPane } = show({ tab: "doc", path: "Creator/P/素材/访谈.md" });
    await waitFor(() => expect(screen.getByTestId("editor").textContent).toBe("Creator/P/素材/访谈.md"));
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.project.closeDoc") }));
    expect(onPane).toHaveBeenCalledWith({ tab: "overview" });
  });

  it("一篇笔记落在哪个页签：主页、研究、脚本各回自己的页签，其余作为临时页签", () => {
    const data = project();
    expect(paneFor(HOME, HOME, data)).toEqual({ tab: "overview" });
    expect(paneFor("Creator/P/P-研究.md", HOME, data)).toEqual({ tab: "research" });
    expect(paneFor("Creator/P/P-脚本.md", HOME, data)).toEqual({ tab: "script" });
    expect(paneFor("Notes/别处.md", HOME, data)).toEqual({ tab: "doc", path: "Notes/别处.md" });
  });

  it("更多 = 项目菜单：重命名交给宿主跟到新主页", async () => {
    setCommandReply("creatorRenameProject", { status: "ok", data: "Creator/新名/新名.md" });
    const { projectNav } = show({ tab: "script" });
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.project.more.label") }));
    const items = (await screen.findAllByRole("menuitem")).map((item) => item.textContent);
    expect(items).toEqual([
      t("workspace:creator.projectMenu.open"),
      t("workspace:creator.projectMenu.openPublish"),
      t("workspace:creator.projectMenu.moveTo"),
      t("workspace:creator.projectMenu.rename"),
      t("common:menu.revealInFileManager"),
      t("workspace:creator.projectMenu.archive"),
      t("workspace:creator.projectMenu.delete"),
    ]);
    fireEvent.click(screen.getByRole("menuitem", { name: t("workspace:creator.projectMenu.rename") }));
    const dialog = within(await screen.findByRole("dialog"));
    fireEvent.change(dialog.getByRole("textbox"), { target: { value: "新名" } });
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.projectMenu.renameConfirm") }));
    await waitFor(() => expect(commandMock("creatorRenameProject")).toHaveBeenCalledWith(HOME, "新名"));
    await waitFor(() => expect(projectNav.onRenamed).toHaveBeenCalledWith(HOME, "Creator/新名/新名.md"));
  });

  it("更多 = 项目菜单：归档后页头写明「已归档」，菜单里换成「取消归档」", async () => {
    setCommandReply("creatorSetArchived", {
      status: "ok",
      data: { view: "board", stages: [], hidden_stages: [], projects: [], archived: [] },
    });
    const { projectNav } = show();
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.project.more.label") }));
    setCommandReply("creatorProject", { status: "ok", data: project({ archived: true }) });
    fireEvent.click(await screen.findByRole("menuitem", { name: t("workspace:creator.projectMenu.archive") }));
    await waitFor(() => expect(commandMock("creatorSetArchived")).toHaveBeenCalledWith(HOME, true));
    // 归档只写主页标记、库版本不变：要宿主重读看板，侧栏才跟得上（真机发现）。
    await waitFor(() => expect(projectNav.onChanged).toHaveBeenCalledWith(HOME));
    expect(await screen.findByText(t("workspace:creator.project.more.archivedStatus"))).toBeTruthy();
    expect(await screen.findByText(t("workspace:creator.projectMenu.archivedNotice"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.project.more.label") }));
    expect(await screen.findByRole("menuitem", { name: t("workspace:creator.projectMenu.restore") })).toBeTruthy();
  });

  it("删除项目：先列出会删什么、写明去向，确认后交给笔记删除同一条路，宿主回看板", async () => {
    setCommandReply("vaultDeleteMode", "vault_trash");
    setCommandReply("creatorProjectDeletion", {
      status: "ok",
      data: { targets: ["Creator/P"], whole_folder: true, notes: 3, other_files: 2 },
    });
    setCommandReply("vaultDelete", { status: "ok", data: { deleted: ["Creator/P"], failed: [] } });
    const { projectNav } = show();
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.project.more.label") }));
    fireEvent.click(await screen.findByRole("menuitem", { name: t("workspace:creator.projectMenu.delete") }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText(t("workspace:creator.projectMenu.deleteTitle", { name: "P" }))).toBeTruthy();
    expect(dialog.getByText(/3/).textContent).toContain("2");
    expect(commandMock("vaultDelete")).not.toHaveBeenCalled();
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.projectMenu.deleteConfirm") }));
    await waitFor(() => expect(commandMock("vaultDelete")).toHaveBeenCalledWith(["Creator/P"], null));
    await waitFor(() => expect(projectNav.onDeleted).toHaveBeenCalledWith(HOME, ["Creator/P"]));
  });

  it("阶段推进提示只建议：点了才改阶段，「先不用」收起这一条", async () => {
    setCommandReply("creatorSetProjectStage", {
      status: "ok",
      data: { view: "board", stages: [], hidden_stages: [], projects: [], archived: [] },
    });
    show({ tab: "overview" }, project({ stage: "idea", next_action: "write_idea", stage_hint: "research" }));
    const research = t("workspace:creator.stage.research");
    expect(await screen.findByText(t("workspace:creator.project.hint.idea"))).toBeTruthy();
    expect(commandMock("creatorSetProjectStage")).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.project.hint.move", { stage: research }) }));
    await waitFor(() => expect(commandMock("creatorSetProjectStage")).toHaveBeenCalledWith(HOME, "research"));
    cleanup();
    show({ tab: "overview" }, project({ stage: "idea", next_action: "write_idea", stage_hint: "research" }));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.project.hint.dismiss") }));
    expect(screen.queryByText(t("workspace:creator.project.hint.idea"))).toBeNull();
  });
});
