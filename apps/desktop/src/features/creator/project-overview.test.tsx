// @vitest-environment happy-dom
/**
 * 「项目里的东西」面板（ADR-162 ⑫ / ADR-165 ⑥ / ADR-167 ③，PRD-19-004 / 044）：项目首页右侧。
 * 归类与顺序都来自 Rust，这里只验分节呈现与动作；「添加来源」的状态由 `useMaterialImport` 给
 * （与页头共用），对话框由宿主（项目工作区）挂，这里的夹具照样挂上。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { ProjectOverviewDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ConfirmHost } from "../../components/ConfirmHost";
import { PROJECT_FILE_DROP_EVENT } from "../../lib/project-file-drop";
import { AddSourcesDialog } from "./AddSourcesDialog";
import { ProjectOverview, type ProjectOverviewProps } from "./ProjectOverview";
import { useMaterialImport } from "./use-material-import";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("processingQueueSnapshot", []);
  // 「从链接导入」没启用：没有视频链接页签（那一页的用例单独开）。
  setCommandReply("appsList", []);
  setCommandReply("listSettings", { status: "ok", data: [] });
});

const OVERVIEW: ProjectOverviewDto = {
  stage: "writing",
  items: [
    { path: "Creator/P/P-脚本.md", title: "P-脚本", modified_ms: 3, group: "scripts", kind: null, linked_from_research: false, linked_from_home: true, links_to_home: false, in_folder: true },
    { path: "Creator/P/素材/访谈.md", title: "访谈", modified_ms: 3, group: "sources", kind: "media", linked_from_research: true, linked_from_home: false, links_to_home: false, in_folder: true },
    { path: "Notes/外部.md", title: "外部", modified_ms: 2, group: "sources", kind: "text", linked_from_research: true, linked_from_home: false, links_to_home: false, in_folder: false },
    { path: "Notes/资料.md", title: "资料", modified_ms: 2, group: "research", kind: null, linked_from_research: false, linked_from_home: true, links_to_home: false, in_folder: false },
    { path: "Creator/Swipe/好开头.md", title: "好开头", modified_ms: 1, group: "swipes", kind: null, linked_from_research: false, linked_from_home: false, links_to_home: true, in_folder: false },
  ],
  exports: ["Creator/P/发布/P.zh.srt"],
};

const onItemMenu = vi.fn();

function Harness({ onOpenNote, onImportQueued }: { onOpenNote: ProjectOverviewProps["onOpenNote"]; onImportQueued: () => void }) {
  const materials = useMaterialImport("Creator/P/P.md", onImportQueued);
  return createElement(
    "div",
    null,
    createElement(ProjectOverview, { home: "Creator/P/P.md", onOpenNote, onItemMenu, materials }),
    materials.addOpen ? createElement(AddSourcesDialog, { home: "Creator/P/P.md", materials }) : null,
  );
}

function renderOverview(
  overview: ProjectOverviewDto,
  { onOpenNote = () => {}, onImportQueued = () => {} }: { onOpenNote?: (path: string) => void; onImportQueued?: () => void } = {},
) {
  setCommandReply("creatorProjectOverview", { status: "ok", data: overview });
  return render(withI18n(createElement(ConfirmHost, null, createElement(Harness, { onOpenNote, onImportQueued }))));
}

describe("项目里的东西", () => {
  it("按 Rust 给的类分节列出，写清每篇跟项目的关系；空的类不出现", async () => {
    renderOverview(OVERVIEW);
    const scripts = within(await screen.findByRole("region", { name: t("workspace:creator.overview.group.scripts") }));
    expect(scripts.getByText("P-脚本")).toBeTruthy();
    expect(scripts.getByText(t("workspace:creator.overview.linkedFromHome"))).toBeTruthy();
    expect(scripts.getByText(t("workspace:creator.overview.inFolder"))).toBeTruthy();
    const swipes = within(screen.getByRole("region", { name: t("workspace:creator.overview.group.swipes") }));
    expect(swipes.getByText(t("workspace:creator.overview.linksToHome"))).toBeTruthy();
    expect(screen.queryByRole("region", { name: t("workspace:creator.overview.group.footage") })).toBeNull();
    expect(commandMock("creatorProjectOverview")).toHaveBeenCalledWith("Creator/P/P.md");
  });

  it("点一篇交给项目工作区打开；导出文件在文件管理器中显示；不再有「打开项目主页」", async () => {
    setCommandReply("revealVaultPath", { status: "ok", data: null });
    const onOpenNote = vi.fn();
    renderOverview(OVERVIEW, { onOpenNote });
    fireEvent.click(await screen.findByText("资料"));
    expect(onOpenNote).toHaveBeenLastCalledWith("Notes/资料.md");
    fireEvent.click(screen.getByText("P.zh.srt"));
    expect(commandMock("revealVaultPath")).toHaveBeenCalledWith("Creator/P/发布/P.zh.srt");
    // 项目首页 = 主页笔记就地编辑 + 这个面板（ADR-165 ⑥）：两个首页只剩一个。
    expect(screen.queryByText("打开项目主页")).toBeNull();
  });

  it("项目里还没有东西：说清怎么放进来", async () => {
    renderOverview({ stage: "idea", items: [], exports: [] });
    expect(await screen.findByText(t("workspace:creator.overview.emptyTitle"))).toBeTruthy();
  });

  it("素材与来源：写明类型与此刻的处理状态；研究笔记链到的项目外笔记标出关系", async () => {
    setCommandReply("processingQueueSnapshot", [
      { id: "t1", note_rel: "Creator/P/素材/访谈.md", status: { state: "running" } },
    ]);
    renderOverview(OVERVIEW);
    const sources = within(await screen.findByRole("region", { name: t("workspace:creator.overview.group.sources") }));
    expect(sources.getByText(t("workspace:creator.overview.kind.media"))).toBeTruthy();
    expect(sources.getByText(t("workspace:creator.overview.kind.text"))).toBeTruthy();
    expect(sources.getByText(t("workspace:creator.overview.linkedFromResearch"))).toBeTruthy();
    // 素材文件夹里的不再重复标「在项目文件夹里」。
    expect(sources.queryByText(t("workspace:creator.overview.inFolder"))).toBeNull();
    expect(await sources.findByText(t("workspace:creator.overview.status.running"))).toBeTruthy();
  });

  it("添加来源 ▸ 文件：音视频与文档一起选，交给 Rust 分流进这个项目的素材文件夹，入队后关上对话框、打开处理队列", async () => {
    setCommandReply("pickMaterialFiles", { status: "ok", data: ["/Users/me/访谈.m4a", "/Users/me/提纲.docx"] });
    setCommandReply("taskEstimate", null);
    setCommandReply("creatorImportMaterial", { status: "ok", data: 2 });
    const onImportQueued = vi.fn();
    renderOverview(OVERVIEW, { onImportQueued });
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.overview.addSources") }));
    const dialog = within(await screen.findByRole("dialog", { name: t("workspace:creator.addSources.title") }));
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.addSources.pickFiles") }));
    await waitFor(() => expect(onImportQueued).toHaveBeenCalledTimes(1));
    expect(commandMock("creatorImportMaterial")).toHaveBeenCalledWith("Creator/P/P.md", [
      "/Users/me/访谈.m4a",
      "/Users/me/提纲.docx",
    ]);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText(t("workspace:creator.overview.materialQueued", { count: 2 }))).toBeTruthy();
  });

  it("把文件拖进面板：同一条管线进这个项目", async () => {
    setCommandReply("taskEstimate", null);
    setCommandReply("creatorImportMaterial", { status: "ok", data: 1 });
    const onImportQueued = vi.fn();
    renderOverview(OVERVIEW, { onImportQueued });
    const panel = (await screen.findByRole("complementary", { name: t("workspace:creator.overview.title") })) as HTMLElement;
    expect(panel.getAttribute("data-project-drop")).toBe("Creator/P/P.md");
    panel.dispatchEvent(
      new CustomEvent(PROJECT_FILE_DROP_EVENT, { detail: { paths: ["/Users/me/b-roll.mov"], home: "Creator/P/P.md" } }),
    );
    await waitFor(() => expect(commandMock("creatorImportMaterial")).toHaveBeenCalledWith("Creator/P/P.md", ["/Users/me/b-roll.mov"]));
    await waitFor(() => expect(onImportQueued).toHaveBeenCalledTimes(1));
  });

  it("添加来源 ▸ 网页：只交出地址，入队后打开处理队列；剪藏关着时给出直达设置的入口", async () => {
    setCommandReply("creatorClipWeb", { status: "ok", data: null });
    const onImportQueued = vi.fn();
    renderOverview(OVERVIEW, { onImportQueued });
    const open = async () => {
      fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.overview.addSources") }));
      const dialog = within(await screen.findByRole("dialog"));
      fireEvent.click(dialog.getByRole("tab", { name: t("workspace:creator.addSources.tab.web") }));
      return dialog;
    };
    let dialog = await open();
    fireEvent.change(dialog.getByLabelText(t("workspace:creator.addSources.webLabel")), {
      target: { value: "https://example.com/post/1" },
    });
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.addSources.clip") }));
    await waitFor(() => expect(onImportQueued).toHaveBeenCalledTimes(1));
    expect(commandMock("creatorClipWeb")).toHaveBeenCalledWith("Creator/P/P.md", "https://example.com/post/1");

    setCommandReply("creatorClipWeb", { status: "error", error: "WEB_CLIP_DISABLED: 网页剪藏已在设置里关闭" });
    dialog = await open();
    fireEvent.change(dialog.getByLabelText(t("workspace:creator.addSources.webLabel")), {
      target: { value: "https://example.com/post/2" },
    });
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.addSources.clip") }));
    expect(await dialog.findByRole("button", { name: t("workspace:creator.addSources.openClipSetting") })).toBeTruthy();
    expect(onImportQueued).toHaveBeenCalledTimes(1);
  });

  it("添加来源 ▸ 粘贴文字与库里的笔记：交给 Rust 存成来源 / 链进项目，然后面板重取", async () => {
    setCommandReply("creatorAddTextSource", { status: "ok", data: "Creator/P/素材/采访提纲.md" });
    setCommandReply("creatorLinkSources", { status: "ok", data: 1 });
    setCommandReply("search", {
      status: "ok",
      data: [
        { path: "Creator/P/P.md", title: "P", score: null, snippet: "", modified_ms: 3, duration_ms: null, cursor: null },
        { path: "Notes/选题库.md", title: "选题库", score: null, snippet: "", modified_ms: 2, duration_ms: null, cursor: null },
      ],
    });
    renderOverview(OVERVIEW);
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.overview.addSources") }));
    let dialog = within(await screen.findByRole("dialog"));
    fireEvent.click(dialog.getByRole("tab", { name: t("workspace:creator.addSources.tab.text") }));
    fireEvent.change(dialog.getByLabelText(t("workspace:creator.addSources.textTitle")), { target: { value: "采访提纲" } });
    fireEvent.change(dialog.getByLabelText(t("workspace:creator.addSources.textBody")), { target: { value: "  一、开场\n二、经历 " } });
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.addSources.addText") }));
    await waitFor(() =>
      expect(commandMock("creatorAddTextSource")).toHaveBeenCalledWith("Creator/P/P.md", "采访提纲", "  一、开场\n二、经历 "),
    );
    await waitFor(() => expect(commandMock("creatorProjectOverview").mock.calls.length).toBeGreaterThan(1));

    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.overview.addSources") }));
    dialog = within(await screen.findByRole("dialog"));
    fireEvent.click(dialog.getByRole("tab", { name: t("workspace:creator.addSources.tab.vault") }));
    // 项目主页自己不列。
    fireEvent.click(await dialog.findByRole("checkbox", { name: /选题库/ }));
    expect(dialog.queryByRole("checkbox", { name: /^P / })).toBeNull();
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.addSources.link", { count: 1 }) }));
    await waitFor(() => expect(commandMock("creatorLinkSources")).toHaveBeenCalledWith("Creator/P/P.md", ["Notes/选题库.md"]));
    expect(await screen.findByText(t("workspace:creator.overview.notesLinked", { count: 1 }))).toBeTruthy();
  });

  it("添加来源 ▸ 视频链接：只在「从链接导入」启用后出现；第一次先确认权利声明，再入队并建进这个项目", async () => {
    setCommandReply("appsList", [
      {
        id: "link_import",
        enabled: true,
        space: null,
        note_types: [],
        permissions: [],
        contributions: [],
        components: [],
      },
    ]);
    setCommandReply("fetchToolStatus", {
      status: "ok",
      data: {
        ready: true,
        tool_path: "/x/yt-dlp",
        version: "2026.10.01",
        ffmpeg: false,
        ffmpeg_path: null,
        strict_local: false,
        probe_timed_out: false,
        managed_version: null,
        using_managed: false,
        js_runtime: null,
        js_runtime_path: null,
        using_managed_js: false,
        managed_js_version: null,
        rights_confirmed: false,
      },
    });
    setCommandReply("fetchInstallPlan", { components: [], install_dir: "/tools" });
    setCommandReply("probeMediaUrl", {
      status: "ok",
      data: {
        site: "YouTube",
        playlist_title: null,
        is_playlist: false,
        entries: [
          { id: "v1", title: "访谈完整版", url: "https://www.youtube.com/watch?v=v1", duration_ms: 60000, uploader: null, filesize: null, is_live: false },
        ],
        truncated: false,
        limit: 50,
      },
    });
    setCommandReply("confirmLinkImportRights", { status: "ok", data: null });
    setCommandReply("enqueueMediaUrl", { status: "ok", data: 1 });
    const onImportQueued = vi.fn();
    renderOverview(OVERVIEW, { onImportQueued });
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.overview.addSources") }));
    const dialog = within(await screen.findByRole("dialog", { name: t("workspace:creator.addSources.title") }));
    fireEvent.click(await dialog.findByRole("tab", { name: t("workspace:creator.addSources.tab.video") }));
    fireEvent.change(dialog.getByLabelText(t("workspace:creator.addSources.videoLabel")), {
      target: { value: "https://www.youtube.com/watch?v=v1" },
    });
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.addSources.videoProbe") }));
    expect(await dialog.findByText("访谈完整版")).toBeTruthy();
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.addSources.videoAdd", { count: 1 }) }));
    const rights = within(await screen.findByRole("alertdialog"));
    expect(commandMock("enqueueMediaUrl")).not.toHaveBeenCalled();
    fireEvent.click(rights.getByRole("button", { name: t("transcribe:fetch.rights.confirm") }));
    await waitFor(() =>
      expect(commandMock("enqueueMediaUrl")).toHaveBeenCalledWith(
        [{ url: "https://www.youtube.com/watch?v=v1", title: "访谈完整版", filesize: null }],
        expect.any(String),
        "Creator/P/P.md",
      ),
    );
    expect(commandMock("confirmLinkImportRights")).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(onImportQueued).toHaveBeenCalledTimes(1));
    expect(screen.getByText(t("workspace:creator.overview.videoQueued", { count: 1 }))).toBeTruthy();
  });

  it("「从链接导入」没启用：没有视频链接页签", async () => {
    renderOverview(OVERVIEW);
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.overview.addSources") }));
    const dialog = within(await screen.findByRole("dialog"));
    expect(dialog.queryByRole("tab", { name: t("workspace:creator.addSources.tab.video") })).toBeNull();
  });

  it("每一行右键与「···」都交给宿主的笔记菜单；研究笔记链进来的项目外笔记多一项「移出项目」", async () => {
    setCommandReply("creatorUnlinkSource", { status: "ok", data: true });
    renderOverview(OVERVIEW);
    const row = (await screen.findByText("好开头")).closest("li") as HTMLElement;
    fireEvent.contextMenu(row);
    expect(onItemMenu).toHaveBeenLastCalledWith(expect.anything(), "Creator/Swipe/好开头.md", undefined);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.overview.more", { name: "好开头" }) }));
    expect(onItemMenu).toHaveBeenCalledTimes(2);

    // 素材文件夹里的：文件就在项目里，没有「移出项目」（删除走笔记菜单），只有「来源指南」。
    const lastLeading = () =>
      onItemMenu.mock.calls[onItemMenu.mock.calls.length - 1]?.[2] as { label: string; onSelect: () => void }[];
    fireEvent.contextMenu(screen.getByText("访谈").closest("li") as HTMLElement);
    expect(lastLeading().map((entry) => entry.label)).toEqual([t("workspace:creator.overview.sourceGuide")]);
    fireEvent.contextMenu(screen.getByText("外部").closest("li") as HTMLElement);
    const leading = lastLeading();
    expect(leading.map((entry) => entry.label)).toEqual([
      t("workspace:creator.overview.sourceGuide"),
      t("workspace:creator.overview.removeFromProject"),
    ]);
    leading[1]!.onSelect();
    await waitFor(() => expect(commandMock("creatorUnlinkSource")).toHaveBeenCalledWith("Creator/P/P.md", "Notes/外部.md"));
    expect(await screen.findByText(t("workspace:creator.overview.removedNotice", { name: "外部" }))).toBeTruthy();
  });

  it("来源指南：从一行的菜单打开，取 Rust 给的摘要与要点；可重新生成", async () => {
    setCommandReply("creatorSourceGuide", {
      status: "ok",
      data: { summary: "一次访谈的转写。", points: ["要点一", "要点二"], cached: true },
    });
    renderOverview(OVERVIEW);
    fireEvent.contextMenu((await screen.findByText("访谈")).closest("li") as HTMLElement);
    const leading = onItemMenu.mock.calls[onItemMenu.mock.calls.length - 1]?.[2] as { onSelect: () => void }[];
    act(() => leading[0]!.onSelect());
    const dialog = within(await screen.findByRole("dialog", { name: t("workspace:creator.research.guideTitle", { name: "访谈" }) }));
    expect(await dialog.findByText("一次访谈的转写。")).toBeTruthy();
    expect(dialog.getByText("要点二")).toBeTruthy();
    expect(dialog.getByText(t("workspace:creator.research.cached"))).toBeTruthy();
    expect(commandMock("creatorSourceGuide")).toHaveBeenLastCalledWith("Creator/P/素材/访谈.md", false);
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.research.regenerate") }));
    await waitFor(() => expect(commandMock("creatorSourceGuide")).toHaveBeenLastCalledWith("Creator/P/素材/访谈.md", true));
  });

  it("写研究简报：默认勾上全部来源，按列出的顺序交给 Rust，写好就在项目里打开", async () => {
    setCommandReply("creatorResearchBrief", { status: "ok", data: "Creator/P/P-研究简报.md" });
    const onOpenNote = vi.fn();
    renderOverview(OVERVIEW, { onOpenNote });
    const sources = within(await screen.findByRole("region", { name: t("workspace:creator.overview.group.sources") }));
    fireEvent.click(sources.getByRole("button", { name: t("workspace:creator.overview.writeBrief") }));
    const dialog = within(await screen.findByRole("dialog", { name: t("workspace:creator.research.briefTitle") }));
    // 取消一条再勾回来：顺序仍按列出的顺序。
    fireEvent.click(dialog.getByRole("checkbox", { name: "访谈" }));
    fireEvent.click(dialog.getByRole("checkbox", { name: "访谈" }));
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.research.write", { count: 2 }) }));
    await waitFor(() =>
      expect(commandMock("creatorResearchBrief")).toHaveBeenCalledWith("Creator/P/P.md", [
        "Creator/P/素材/访谈.md",
        "Notes/外部.md",
      ]),
    );
    await waitFor(() => expect(onOpenNote).toHaveBeenCalledWith("Creator/P/P-研究简报.md"));
  });

  it("从简报出发：提三个选题，选一个改一改写进主页；起草脚本另存时打开草稿并说明原脚本没动", async () => {
    setCommandReply("creatorBriefTopics", {
      status: "ok",
      data: [
        { title: "选题一", angle: "切入一" },
        { title: "选题二", angle: "切入二" },
        { title: "选题三", angle: "" },
      ],
    });
    setCommandReply("creatorSetTopic", { status: "ok", data: null });
    setCommandReply("creatorDraftScript", { status: "ok", data: { path: "Creator/P/P-脚本初稿.md", into_script: false } });
    const onOpenNote = vi.fn();
    renderOverview(OVERVIEW, { onOpenNote });
    const sources = within(await screen.findByRole("region", { name: t("workspace:creator.overview.group.sources") }));
    fireEvent.click(sources.getByRole("button", { name: t("workspace:creator.overview.fromBrief") }));
    let dialog = within(await screen.findByRole("dialog", { name: t("workspace:creator.research.nextTitle") }));
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.research.suggest") }));
    fireEvent.click(await dialog.findByRole("radio", { name: /选题二/ }));
    const field = dialog.getByRole("textbox", { name: t("workspace:creator.research.topicText") }) as HTMLInputElement;
    expect(field.value).toBe("选题二");
    fireEvent.change(field, { target: { value: "选题二，讲给新手" } });
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.research.saveTopic") }));
    await waitFor(() => expect(commandMock("creatorSetTopic")).toHaveBeenCalledWith("Creator/P/P.md", "选题二，讲给新手"));
    expect(await screen.findByText(t("workspace:creator.overview.topicSaved"))).toBeTruthy();

    fireEvent.click(sources.getByRole("button", { name: t("workspace:creator.overview.fromBrief") }));
    dialog = within(await screen.findByRole("dialog", { name: t("workspace:creator.research.nextTitle") }));
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.research.draft") }));
    await waitFor(() => expect(onOpenNote).toHaveBeenCalledWith("Creator/P/P-脚本初稿.md"));
    expect(commandMock("creatorDraftScript")).toHaveBeenCalledWith("Creator/P/P.md");
    expect(await screen.findByText(t("workspace:creator.overview.scriptDraftAside"))).toBeTruthy();
  });

  it("从简报出发：还没有研究简报时把 Rust 的说明显示在对话框里", async () => {
    setCommandReply("creatorBriefTopics", { status: "error", error: "CREATOR_RESEARCH_NO_BRIEF: 这个项目还没有研究简报。" });
    renderOverview(OVERVIEW);
    const sources = within(await screen.findByRole("region", { name: t("workspace:creator.overview.group.sources") }));
    fireEvent.click(sources.getByRole("button", { name: t("workspace:creator.overview.fromBrief") }));
    const dialog = within(await screen.findByRole("dialog", { name: t("workspace:creator.research.nextTitle") }));
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.research.suggest") }));
    expect((await dialog.findByRole("alert")).textContent).toContain("还没有研究简报");
    expect(dialog.queryByRole("radio")).toBeNull();
  });
});
