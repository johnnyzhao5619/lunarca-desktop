// @vitest-environment happy-dom
/**
 * 「分发」页签（ADR-165 ⑧，PRD-19-024 / 025）：改写来源、生成后在项目里打开、已有的列出来；
 * 短视频只从成片转写挑，没有成片时说清要先导入。写哪篇、叫什么、对齐时间点都在 Rust。
 * 按目标渠道（ADR-166 ④ S3）：每一行是 Rust 给的一个目标，已有的那份带着超限的地方。
 */
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { ChannelDto, DistributionDto, PublishSourceDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { DistributeView } from "./DistributeView";

const HOME = "Creator/P/P.md";
const CUT: PublishSourceDto = { path: "Creator/P/成片.md", title: "成片", modified_ms: 1, duration_secs: 600, has_translation: false };

/** 项目里没有文章 / 帖子 / 简报 / 短视频渠道：每种形态一个通用目标。 */
const GENERIC: DistributionDto["targets"] = (["blog", "newsletter", "thread", "shorts"] as const).map((kind) => ({
  kind,
  channel: null,
}));

const NO_LIMITS = { title_max: null, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null };
const CATALOG: ChannelDto[] = [
  { id: "x", name: null, format: "post", limits: { ...NO_LIMITS, body_max: 280 }, style: null, builtin: true, ai_notice: false },
  { id: "linkedin", name: null, format: "post", limits: { ...NO_LIMITS, body_max: 3000 }, style: null, builtin: true, ai_notice: false },
  { id: "douyin", name: null, format: "short_video", limits: NO_LIMITS, style: null, builtin: true, ai_notice: true },
];

function show(
  script: string | null,
  sources: PublishSourceDto[],
  targets: DistributionDto["targets"] = GENERIC,
  outputs: DistributionDto["outputs"] = [{ kind: "blog", channel: null, path: "Creator/P/P-博客.md", issues: [] }],
) {
  setCommandReply("creatorDistribution", { status: "ok", data: { script, targets, outputs } });
  setCommandReply("creatorChannels", { status: "ok", data: CATALOG });
  setCommandReply("creatorPublishSources", { status: "ok", data: sources });
  const onOpenNote = vi.fn();
  const onGoToPublish = vi.fn();
  render(withI18n(createElement(DistributeView, { home: HOME, onOpenNote, onGoToPublish })));
  return { onOpenNote, onGoToPublish };
}

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("分发页签", () => {
  it("默认从脚本改写；生成完在项目里打开；已有的一份可以直接打开", async () => {
    setCommandReply("creatorDistribute", { status: "ok", data: "Creator/P/P-简报.md" });
    const { onOpenNote } = show("Creator/P/P-脚本.md", [CUT]);
    const script = await screen.findByRole("radio", { name: t("workspace:creator.distribute.fromScript") });
    expect(script.getAttribute("aria-checked")).toBe("true");
    const buttons = screen.getAllByRole("button", { name: t("workspace:creator.distribute.generate") });
    fireEvent.click(buttons[1]);
    await waitFor(() =>
      expect(commandMock("creatorDistribute")).toHaveBeenCalledWith(HOME, "newsletter", null, { kind: "script" }, null),
    );
    await waitFor(() => expect(onOpenNote).toHaveBeenCalledWith("Creator/P/P-简报.md"));
    fireEvent.click(screen.getByRole("button", { name: "P-博客" }));
    expect(onOpenNote).toHaveBeenLastCalledWith("Creator/P/P-博客.md");
  });

  it("改从成片转写改写；短视频总是从成片转写挑", async () => {
    setCommandReply("creatorDistribute", { status: "ok", data: "Creator/P/P-短视频.md" });
    show("Creator/P/P-脚本.md", [CUT]);
    fireEvent.click(await screen.findByRole("radio", { name: "成片" }));
    const buttons = screen.getAllByRole("button", { name: t("workspace:creator.distribute.generate") });
    fireEvent.click(buttons[0]);
    await waitFor(() =>
      expect(commandMock("creatorDistribute")).toHaveBeenCalledWith(HOME, "blog", null, { kind: "transcript", path: CUT.path }, null),
    );
    fireEvent.click(buttons[3]);
    await waitFor(() =>
      expect(commandMock("creatorDistribute")).toHaveBeenLastCalledWith(HOME, "shorts", null, { kind: "transcript", path: CUT.path }, null),
    );
  });

  it("没有成片：短视频不可生成，写明要先导入并给去处；失败如实说", async () => {
    setCommandReply("creatorDistribute", { status: "error", error: "MODEL_UNAVAILABLE: x" });
    const { onGoToPublish } = show("Creator/P/P-脚本.md", []);
    await screen.findByText(t("workspace:creator.distribute.needsCut"));
    const buttons = screen.getAllByRole("button", { name: t("workspace:creator.distribute.generate") });
    expect((buttons[3] as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.distribute.goToPublish") }));
    expect(onGoToPublish).toHaveBeenCalledTimes(1);
    fireEvent.click(buttons[2]);
    expect(await screen.findByRole("alert")).toBeTruthy();
  });

  it("项目里的渠道各一行：按那个渠道生成；已有的那份列出超出上限的地方", async () => {
    setCommandReply("creatorDistribute", { status: "ok", data: "Creator/P/P-LinkedIn.md" });
    show(
      "Creator/P/P-脚本.md",
      [CUT],
      [
        { kind: "blog", channel: null },
        { kind: "newsletter", channel: null },
        { kind: "thread", channel: "x" },
        { kind: "thread", channel: "linkedin" },
        { kind: "shorts", channel: "douyin" },
      ],
      [{ kind: "thread", channel: "x", path: "Creator/P/P-X.md", issues: [{ kind: "post", post: 2, over: 7, max: 280, weighted: true }] }],
    );
    const kind = t("workspace:creator.distribute.kind.thread");
    const x = t("workspace:creator.distribute.target", { channel: t("workspace:creator.channel.x"), kind });
    expect(await screen.findByText(x)).toBeTruthy();
    expect(
      screen.getByText(t("workspace:creator.distribute.target", { channel: t("workspace:creator.channel.douyin"), kind: t("workspace:creator.distribute.kind.shorts") })),
    ).toBeTruthy();
    // 通用的推文串一行没有了：项目里有帖子渠道。
    expect(screen.queryByText(kind)).toBeNull();
    // X 按加权字数算：提示里说明中日韩文字算两个。
    expect(screen.getByText(t("workspace:creator.distribute.issue.postWeighted", { post: 2, over: 7, max: 280 }))).toBeTruthy();
    // 这次的额外要求随这一页的生成一起发（去掉首尾空白）。
    fireEvent.change(screen.getByRole("textbox", { name: t("workspace:creator.extra.label") }), {
      target: { value: "  面向新手 " },
    });
    const buttons = screen.getAllByRole("button", { name: t("workspace:creator.distribute.generate") });
    fireEvent.click(buttons[3]);
    await waitFor(() =>
      expect(commandMock("creatorDistribute")).toHaveBeenCalledWith(HOME, "thread", "linkedin", { kind: "script" }, "面向新手"),
    );
  });
});
