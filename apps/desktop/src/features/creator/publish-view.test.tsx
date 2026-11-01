// @vitest-environment happy-dom
/**
 * 发布包页（ADR-162 ⑪，PRD-19-018~020）：成片候选、字幕一次出齐。
 * 写哪几份、文件名、简繁转换都在 Rust；这里只验命令的入参与结果的呈现。
 * 分步清单（ADR-165 ⑧，PRD-19-039）：步骤、完成度、前置与检查都按 Rust 给的那一份渲染。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type {
  PublishChecklistDto,
  PublishStepDto,
  PublishStepStateDto,
} from "../../ipc/bindings";
import type { ChannelDto, PublishSourceDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { PublishView } from "./PublishView";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  // 译成哪种语言的选项来自设置注册表；这里不关心有哪些，给空表即可。
  setCommandReply("listSettings", { status: "ok", data: [] });
  // 边改边查的默认回答：没有问题。
  setCommandReply("creatorCheckChapters", { issues: [], fixed: null });
  setCommandReply("creatorCheckPackage", { status: "ok", data: [] });
  setCommandReply("creatorPackage", { status: "ok", data: null });
  setCommandReply("creatorChannels", { status: "ok", data: CATALOG });
  // 「我的渠道」：YouTube 与 B 站，都是新项目默认。
  setCommandReply("creatorChannelFavorites", { status: "ok", data: FAVORITES });
});
const FAVORITES = [
  { id: "youtube", default: true },
  { id: "bilibili", default: true },
];
const CATALOG: ChannelDto[] = [
  { id: "youtube", name: null, format: "long_video", limits: { title_max: 100, body_max: 5000, tags_max: null, tag_max: null, clip_max_secs: null }, style: null, builtin: true, ai_notice: true },
  { id: "bilibili", name: null, format: "long_video", limits: { title_max: 80, body_max: 2000, tags_max: 10, tag_max: 20, clip_max_secs: null }, style: null, builtin: true, ai_notice: true },
  { id: "douyin", name: null, format: "short_video", limits: { title_max: null, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null }, style: null, builtin: true, ai_notice: true },
  { id: "podcast", name: null, format: "audio", limits: { title_max: null, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null }, style: null, builtin: true, ai_notice: true },
];

/** 测试里点名一步：`package:<渠道>` 是那个渠道的包装。 */
type StepName = "final_cut" | "subtitles" | "chapters" | "check" | `package:${string}`;

function stepOf(name: StepName): PublishStepDto {
  return name.startsWith("package:")
    ? { kind: "package", channel: name.slice("package:".length) }
    : { kind: name as "final_cut" | "subtitles" | "chapters" | "check" };
}

/** 一步在界面上的名字（渠道包装带渠道名）。 */
function stepName(name: StepName): string {
  return name.startsWith("package:")
    ? t("workspace:creator.publish.step.package", {
        channel: t(`workspace:creator.channel.${name.slice("package:".length)}`),
      })
    : t(`workspace:creator.publish.step.${name}`);
}


/** 页面上的某一节（按它的标题找），同名按钮在不同节里各有一个。 */
function section(titleKey: string) {
  return within(screen.getByRole("region", { name: t(titleKey) }));
}

const CUT: PublishSourceDto = {
  path: "Creator/P/成片.md",
  title: "成片定稿",
  modified_ms: 2,
  duration_secs: 75,
  has_translation: true,
};

/** Rust 给的清单：默认 YouTube 与 B 站都发、每步都没做、不缺成片；短视频渠道没有包装一步。 */
function checklist(
  channels: string[] = ["youtube", "bilibili"],
  patch: Partial<Record<StepName, Partial<PublishStepStateDto>>> = {},
  checks: PublishChecklistDto["checks"] = [],
): PublishChecklistDto {
  const order: StepName[] = [
    "final_cut",
    "subtitles",
    "chapters",
    ...channels.filter((channel) => channel !== "douyin").map((channel) => `package:${channel}` as const),
    "check",
  ];
  const steps = order.map((name) => ({ step: stepOf(name), done: false, blocked_by: null, issues: 0, ...patch[name] }));
  return {
    channels,
    steps,
    done: steps.filter((step) => step.done).length,
    total: steps.length,
    checks,
  };
}

/** 点步骤列里的一步。 */
function goToStep(name: StepName) {
  const nav = within(screen.getByRole("navigation", { name: t("workspace:creator.publish.stepsLabel") }));
  fireEvent.click(nav.getByText(stepName(name)));
}

function renderView(
  sources: PublishSourceDto[],
  onOpenNote: (path: string) => void = () => {},
  onImportQueued: () => void = () => {},
  list: PublishChecklistDto = checklist(),
) {
  setCommandReply("creatorPublishSources", { status: "ok", data: sources });
  setCommandReply("creatorPublishChecklist", { status: "ok", data: list });
  return render(
    withI18n(
      createElement(ConfirmHost, null, createElement(ContextMenuHost, null, createElement(PublishView, {
          home: "Creator/P/P.md",
          onImportQueued,
          onOpenNote,
        })),
      ),
    ),
  );
}

describe("发布包页", () => {
  it("列出成片，按选定的格式、字形与 BCC 一次导出，并列出写了哪些文件", async () => {
    setCommandReply("creatorExportSubtitles", {
      status: "ok",
      data: { folder: "Creator/P/发布", files: ["Creator/P/发布/P.zh-Hant-TW.srt", "Creator/P/发布/P.en.srt"] },
    });
    renderView([CUT]);
    expect(await screen.findByText("成片定稿")).toBeTruthy();
    expect(screen.getByText("1:15")).toBeTruthy();
    goToStep("subtitles");
    fireEvent.click(screen.getByRole("radio", { name: t("transcribe:exportOptions.han_traditional_taiwan") }));
    fireEvent.click(screen.getByRole("checkbox", { name: t("workspace:creator.publish.bcc") }));
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.export") }));
    await waitFor(() => expect(screen.getByText("P.en.srt")).toBeTruthy());
    expect(commandMock("creatorExportSubtitles")).toHaveBeenCalledWith(
      "Creator/P/P.md",
      "Creator/P/成片.md",
      "srt",
      true,
      "traditional_taiwan",
    );
    expect(screen.getByText("P.zh-Hant-TW.srt")).toBeTruthy();
  });

  it("没有译文时给「翻译」，说清只会导出原文", async () => {
    setCommandReply("enqueueTranslate", { status: "ok", data: null });
    renderView([{ ...CUT, has_translation: false }]);
    await screen.findByText(t("workspace:creator.publish.translateHint"));
    goToStep("subtitles");
    expect(screen.getByText(t("workspace:creator.publish.subtitlesSourceOnly"))).toBeTruthy();
    goToStep("final_cut");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.translate") }));
    await waitFor(() => expect(screen.getByText(t("workspace:creator.publish.translateQueued"))).toBeTruthy());
    expect(commandMock("enqueueTranslate")).toHaveBeenCalledWith("Creator/P/成片.md", "smart");
  });

  it("导入成片：选好的文件交给 Rust 建进这个项目，入队后打开处理队列", async () => {
    setCommandReply("pickMediaFiles", { status: "ok", data: ["/Users/me/成片.mp4"] });
    setCommandReply("taskEstimate", null);
    setCommandReply("creatorImportFinalCut", { status: "ok", data: 1 });
    const onImportQueued = vi.fn();
    renderView([], () => {}, onImportQueued);
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.publish.importFinalCut") }));
    await waitFor(() => expect(onImportQueued).toHaveBeenCalledTimes(1));
    expect(commandMock("creatorImportFinalCut")).toHaveBeenCalledWith("Creator/P/P.md", ["/Users/me/成片.mp4"]);
    expect(screen.getByText(t("workspace:creator.publish.importQueued", { count: 1 }))).toBeTruthy();
  });

  it("导入成片：选了又取消，什么都不排", async () => {
    setCommandReply("pickMediaFiles", { status: "ok", data: [] });
    const onImportQueued = vi.fn();
    renderView([CUT], () => {}, onImportQueued);
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.publish.importFinalCut") }));
    await waitFor(() => expect(commandMock("pickMediaFiles")).toHaveBeenCalledTimes(1));
    expect(commandMock("creatorImportFinalCut")).not.toHaveBeenCalled();
    expect(onImportQueued).not.toHaveBeenCalled();
  });

  it("项目里还没有成片：说清怎么放进来，导出按钮不可用", async () => {
    renderView([]);
    expect(await screen.findByText(t("workspace:creator.publish.noSources"))).toBeTruthy();
    goToStep("subtitles");
    expect(
      (screen.getByRole("button", { name: t("workspace:creator.publish.export") }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe("发布包页 · 章节", () => {
  it("生成后可直接改的文字与问题都来自 Rust；能修的给「按建议修正」；写进发布笔记后可打开", async () => {
    setCommandReply("creatorGenerateChapters", {
      status: "ok",
      data: {
        text: "0:05 开场\n0:20 正文",
        check: { issues: [{ kind: "first_not_at_zero" }, { kind: "too_few", count: 2 }], fixed: "0:00 开场\n0:20 正文" },
        duration_ms: 75000,
      },
    });
    setCommandReply("creatorCheckChapters", {
      issues: [{ kind: "too_few", count: 2 }],
      fixed: null,
    });
    setCommandReply("creatorSaveChapters", { status: "ok", data: "Creator/P/P-发布.md" });
    const onOpenNote = vi.fn();
    renderView([CUT], onOpenNote);
    await screen.findByText("成片定稿");
    goToStep("chapters");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.chapters.generate") }));
    const box = (await screen.findByDisplayValue(/0:05 开场/)) as HTMLTextAreaElement;
    expect(commandMock("creatorGenerateChapters")).toHaveBeenCalledWith("Creator/P/P.md", "Creator/P/成片.md", null);
    expect(screen.getByText(t("workspace:creator.publish.chapters.issue.firstNotAtZero"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.chapters.fix") }));
    expect(box.value).toBe("0:00 开场\n0:20 正文");
    // 改过的文字按停手后的检查结果显示问题（时长取自成片）。
    await waitFor(() => expect(commandMock("creatorCheckChapters")).toHaveBeenCalledWith("0:00 开场\n0:20 正文", 75000));
    await waitFor(() =>
      expect(screen.getByText(t("workspace:creator.publish.chapters.issue.tooFew", { count: 2 }))).toBeTruthy(),
    );
    const chapters = section("workspace:creator.publish.chapters.title");
    fireEvent.click(chapters.getByRole("button", { name: t("workspace:creator.publish.common.save") }));
    fireEvent.click(await chapters.findByRole("button", { name: t("workspace:creator.publish.common.open") }));
    expect(commandMock("creatorSaveChapters")).toHaveBeenCalledWith("Creator/P/P.md", "0:00 开场\n0:20 正文");
    expect(onOpenNote).toHaveBeenCalledWith("Creator/P/P-发布.md");
  });

  it("模型不可用：如实报错，文字框保持空", async () => {
    setCommandReply("creatorGenerateChapters", { status: "error", error: "CREATOR_CHAPTERS_PARSE: x" });
    renderView([CUT]);
    await screen.findByText("成片定稿");
    goToStep("chapters");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.chapters.generate") }));
    await waitFor(() =>
      expect((screen.getByRole("textbox", { name: t("workspace:creator.publish.chapters.title") }) as HTMLTextAreaElement).value).toBe(""),
    );
    expect(commandMock("creatorGenerateChapters")).toHaveBeenCalledTimes(1);
  });

  it("离开页面再回来，没写进笔记的章节文字还在", async () => {
    setCommandReply("creatorCheckChapters", { issues: [], fixed: null });
    const first = renderView([CUT]);
    await screen.findByText("成片定稿");
    goToStep("chapters");
    const box = screen.getByRole("textbox", { name: t("workspace:creator.publish.chapters.title") });
    fireEvent.change(box, { target: { value: "0:00 草稿" } });
    first.unmount();
    renderView([CUT]);
    await screen.findByText("成片定稿");
    goToStep("chapters");
    expect(await screen.findByDisplayValue("0:00 草稿")).toBeTruthy();
  });
});

describe("发布包页 · 渠道包装", () => {
  const PACKAGE = {
    titles: "标题一\n标题二\n标题三",
    description: "这期讲怎么写脚本。\n\n章节\n0:00 开场",
    cover: "三分钟\n看懂脚本\n第三行",
    category: "知识 › 科学科普",
    tags: "科普\n脚本",
  };

  it("每个发布渠道一步：按那个渠道的上限生成、边改边查、写进发布笔记那个渠道的一节", async () => {
    setCommandReply("creatorGeneratePackage", {
      status: "ok",
      data: { package: PACKAGE, issues: [{ kind: "too_many_cover_lines", count: 3 }] },
    });
    setCommandReply("creatorSavePackage", { status: "ok", data: "Creator/P/P-发布.md" });
    renderView([CUT]);
    await screen.findByText("成片定稿");
    goToStep("package:bilibili");
    const area = section(stepName("package:bilibili"));
    // 上限一条一句，只写这个渠道有的。
    const limits = ["title", "description", "tags", "tag"].map((key, index) =>
      t(`workspace:creator.publish.package.limit.${key}`, { count: [80, 2000, 10, 20][index] }),
    );
    expect(
      area.getByText(
        t("workspace:creator.publish.package.limits", {
          list: limits.join(t("workspace:creator.publish.package.limitSeparator")),
        }),
      ),
    ).toBeTruthy();
    fireEvent.change(area.getByRole("textbox", { name: t("workspace:creator.extra.label") }), {
      target: { value: "标题别用问句" },
    });
    fireEvent.click(area.getByRole("button", { name: t("workspace:creator.publish.package.generate") }));
    expect(await area.findByDisplayValue("知识 › 科学科普")).toBeTruthy();
    expect(commandMock("creatorGeneratePackage")).toHaveBeenCalledWith(
      "Creator/P/P.md",
      "Creator/P/成片.md",
      "bilibili",
      "标题别用问句",
    );
    expect(area.getByText(t("workspace:creator.publish.package.issue.tooManyCoverLines", { count: 3 }))).toBeTruthy();

    setCommandReply("creatorCheckPackage", { status: "ok", data: [{ kind: "tag_too_long", tag: 2, over: 4, max: 20 }] });
    const tags = "科普\n一个特别特别特别长的标签名字超出上限了吧真的";
    fireEvent.change(area.getByRole("textbox", { name: t("workspace:creator.publish.package.tags") }), {
      target: { value: tags },
    });
    await waitFor(() => expect(commandMock("creatorCheckPackage")).toHaveBeenCalledWith("bilibili", { ...PACKAGE, tags }));
    await waitFor(() =>
      expect(
        area.getByText(t("workspace:creator.publish.package.issue.tagTooLong", { tag: 2, over: 4, max: 20 })),
      ).toBeTruthy(),
    );
    fireEvent.click(area.getByRole("button", { name: t("workspace:creator.publish.common.save") }));
    await waitFor(() =>
      expect(commandMock("creatorSavePackage")).toHaveBeenCalledWith("Creator/P/P.md", "bilibili", { ...PACKAGE, tags }),
    );
    expect(
      await area.findByText(
        t("workspace:creator.publish.package.saved", { note: "P-发布", section: stepName("package:bilibili") }),
      ),
    ).toBeTruthy();
  });

  it("打开时从发布笔记里那个渠道的一节填回；两个渠道的草稿互不串", async () => {
    setCommandReply("creatorPackage", { status: "ok", data: { ...PACKAGE, titles: "已写的标题" } });
    renderView([CUT]);
    await screen.findByText("成片定稿");
    goToStep("package:youtube");
    const youtube = section(stepName("package:youtube"));
    expect(await youtube.findByDisplayValue("已写的标题")).toBeTruthy();
    expect(commandMock("creatorPackage")).toHaveBeenCalledWith("Creator/P/P.md", "youtube");
    expect(commandMock("creatorPackage")).toHaveBeenCalledWith("Creator/P/P.md", "bilibili");
    fireEvent.change(youtube.getByRole("textbox", { name: t("workspace:creator.publish.package.titles") }), {
      target: { value: "只改 YouTube" },
    });
    goToStep("package:bilibili");
    const bilibili = section(stepName("package:bilibili"));
    expect(
      (bilibili.getByRole("textbox", { name: t("workspace:creator.publish.package.titles") }) as HTMLTextAreaElement).value,
    ).toBe("已写的标题");
  });

  it("没有确定上限的渠道写明这里不核对长度；短视频渠道不在发布清单里", async () => {
    renderView([CUT], () => {}, () => {}, checklist(["podcast", "douyin"]));
    await screen.findByText("成片定稿");
    const nav = within(screen.getByRole("navigation", { name: t("workspace:creator.publish.stepsLabel") }));
    expect(nav.queryByText(stepName("package:douyin"))).toBeNull();
    goToStep("package:podcast");
    expect(
      section(stepName("package:podcast")).getByText(t("workspace:creator.publish.package.noLimits")),
    ).toBeTruthy();
  });
});

describe("发布包页 · 分步清单", () => {
  it("步骤列按 Rust 的清单渲染：页头写完成几步，停在第一个没做完的那一步", async () => {
    renderView(
      [CUT],
      () => {},
      () => {},
      checklist(["youtube", "bilibili"], { final_cut: { done: true }, chapters: { done: true, issues: 2 } }),
    );
    expect(await screen.findByText(t("workspace:creator.publish.check.progress", { done: 2, total: 6 }))).toBeTruthy();
    const nav = within(screen.getByRole("navigation", { name: t("workspace:creator.publish.stepsLabel") }));
    const current = nav.getByText(t("workspace:creator.publish.step.subtitles")).closest("button");
    expect(current?.getAttribute("aria-current")).toBe("step");
    expect(nav.getByText(t("workspace:creator.publish.check.issues", { count: 2 }))).toBeTruthy();
  });

  it("渠道胶囊：去掉一个写回项目主页并重判；只剩一个时去不掉", async () => {
    setCommandReply("creatorSetChannels", { status: "ok", data: null });
    renderView([CUT]);
    const bilibili = t("workspace:creator.channel.bilibili");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.channels.remove", { name: bilibili }) }));
    await waitFor(() => expect(commandMock("creatorSetChannels")).toHaveBeenCalledWith("Creator/P/P.md", ["youtube"]));
    await waitFor(() => expect(commandMock("creatorPublishChecklist").mock.calls.length).toBeGreaterThan(1));
    cleanup();
    renderView([CUT], () => {}, () => {}, checklist(["youtube"]));
    const youtube = t("workspace:creator.channel.youtube");
    const remove = (await screen.findByRole("button", {
      name: t("workspace:creator.channels.remove", { name: youtube }),
    })) as HTMLButtonElement;
    expect(remove.disabled).toBe(true);
  });

  it("添加渠道先列「我的渠道」里还没加的，其余收进「其他渠道」，最后是「管理渠道…」", async () => {
    setCommandReply("creatorSetChannels", { status: "ok", data: null });
    renderView([CUT], () => {}, () => {}, checklist(["youtube"]));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.channels.add") }));
    const items = (await screen.findAllByRole("menuitem")).map((item) => item.textContent);
    expect(items).toEqual([
      t("workspace:creator.channel.bilibili"),
      t("workspace:creator.channels.others"),
      t("workspace:creator.channels.manage"),
    ]);
    fireEvent.click(screen.getByRole("menuitem", { name: t("workspace:creator.channel.bilibili") }));
    await waitFor(() => expect(commandMock("creatorSetChannels")).toHaveBeenCalledWith("Creator/P/P.md", ["youtube", "bilibili"]));
  });

  it("管理渠道：勾常用、勾新项目默认都存进「我的渠道」；新建的自定义渠道存好回到管理框", async () => {
    setCommandReply("creatorSetChannelFavorites", { status: "ok", data: FAVORITES });
    setCommandReply("creatorSaveChannel", {
      status: "ok",
      data: {
        id: "custom-1a2b3c4d",
        name: "我的博客",
        format: "article",
        limits: { title_max: 30, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null },
        style: "",
        builtin: false,
        ai_notice: false,
      },
    });
    renderView([CUT]);
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.channels.add") }));
    fireEvent.click(await screen.findByRole("menuitem", { name: t("workspace:creator.channels.manage") }));
    const manager = within(await screen.findByRole("dialog", { name: t("workspace:creator.channels.managerTitle") }));
    const douyin = t("workspace:creator.channel.douyin");
    expect(
      (manager.getByRole("checkbox", { name: t("workspace:creator.channels.defaultFor", { name: douyin }) }) as HTMLInputElement).disabled,
      "不常用的渠道谈不上新项目默认",
    ).toBe(true);
    fireEvent.click(manager.getByRole("checkbox", { name: douyin }));
    await waitFor(() =>
      expect(commandMock("creatorSetChannelFavorites")).toHaveBeenLastCalledWith([...FAVORITES, { id: "douyin", default: false }]),
    );
    const bilibili = t("workspace:creator.channel.bilibili");
    fireEvent.click(manager.getByRole("checkbox", { name: t("workspace:creator.channels.defaultFor", { name: bilibili }) }));
    await waitFor(() =>
      expect(commandMock("creatorSetChannelFavorites")).toHaveBeenLastCalledWith([
        { id: "youtube", default: true },
        { id: "bilibili", default: false },
      ]),
    );

    fireEvent.click(manager.getByRole("button", { name: t("workspace:creator.channels.new") }));
    const editor = within(await screen.findByRole("dialog", { name: t("workspace:creator.channels.newTitle") }));
    fireEvent.change(editor.getByRole("textbox", { name: t("workspace:creator.channels.name") }), {
      target: { value: "我的博客" },
    });
    fireEvent.change(editor.getByRole("combobox", { name: t("workspace:creator.channels.format") }), {
      target: { value: "article" },
    });
    fireEvent.change(editor.getByRole("textbox", { name: t("workspace:creator.channels.limit.title_max") }), {
      target: { value: "30" },
    });
    fireEvent.click(editor.getByRole("button", { name: t("workspace:creator.channels.save") }));
    await waitFor(() =>
      expect(commandMock("creatorSaveChannel")).toHaveBeenCalledWith({
        id: null,
        name: "我的博客",
        format: "article",
        limits: { title_max: 30, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null },
        style: "",
      }),
    );
    expect(await screen.findByRole("dialog", { name: t("workspace:creator.channels.managerTitle") })).toBeTruthy();
  });

  it("前置不满足的步骤写明缺什么，点按钮回到成片", async () => {
    renderView(
      [],
      () => {},
      () => {},
      checklist(["youtube"], { chapters: { blocked_by: { kind: "final_cut" } } }),
    );
    await screen.findByText(t("workspace:creator.publish.noSources"));
    goToStep("chapters");
    const stepLabel = t("workspace:creator.publish.step.final_cut");
    expect(screen.getByText(t("workspace:creator.publish.check.blocked", { step: stepLabel }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.check.goTo", { step: stepLabel }) }));
    expect(screen.getByText(t("workspace:creator.publish.noSources"))).toBeTruthy();
  });

  it("检查汇总每一条，点一条回到它所属的那一步", async () => {
    renderView(
      [CUT],
      () => {},
      () => {},
      checklist(["youtube"], {}, [
        { kind: "missing", step: { kind: "subtitles" } },
        { kind: "chapters", issue: { kind: "first_not_at_zero" } },
        { kind: "package", channel: "youtube", issue: { kind: "title_count", count: 2 } },
      ]),
    );
    await screen.findByText("成片定稿");
    goToStep("check");
    const chapters = t("workspace:creator.publish.step.chapters");
    expect(
      screen.getByText(
        t("workspace:creator.publish.check.issue", {
          step: chapters,
          text: t("workspace:creator.publish.chapters.issue.firstNotAtZero"),
        }),
      ),
    ).toBeTruthy();
    const youtube = stepName("package:youtube");
    expect(
      screen.getByText(
        t("workspace:creator.publish.check.issue", {
          step: youtube,
          text: t("workspace:creator.publish.package.issue.titleCount", { count: 2 }),
        }),
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.check.goTo", { step: youtube }) }));
    expect(screen.getByRole("navigation", { name: t("workspace:creator.publish.stepsLabel") }).querySelector('[aria-current="step"]')?.textContent).toContain(youtube);
    goToStep("check");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.publish.check.goTo", { step: chapters }) }));
    expect(screen.getByRole("button", { name: t("workspace:creator.publish.chapters.generate") })).toBeTruthy();
  });
});
