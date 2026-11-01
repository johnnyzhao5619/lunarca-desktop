// @vitest-environment happy-dom
/**
 * 新建项目对话框（ADR-165 评审遗留 S6c）：名字、渠道（初值 = Rust 给的默认渠道，至少留一个）、
 * 计划发布日期（可不填），一并交给宿主去建。
 */
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { ChannelDto } from "../../ipc/bindings";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NewProjectDialog } from "./NewProjectDialog";

const NO_LIMITS = { title_max: null, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null };
const CATALOG: ChannelDto[] = ["youtube", "bilibili", "douyin"].map((id) => ({
  id,
  name: null,
  format: id === "douyin" ? "short_video" : "long_video",
  limits: NO_LIMITS,
  style: null,
  builtin: true,
  ai_notice: false,
}));

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("creatorChannels", { status: "ok", data: CATALOG });
  // 「我的渠道」是 YouTube 与 B 站（都是新项目默认）；抖音在「其他渠道」里。
  setCommandReply("creatorChannelFavorites", {
    status: "ok",
    data: [
      { id: "youtube", default: true },
      { id: "bilibili", default: true },
    ],
  });
  setCommandReply("creatorDefaultChannels", { status: "ok", data: ["youtube", "bilibili"] });
});
afterEach(cleanup);

function show(onCreate = vi.fn(), error: string | null = null) {
  render(
    withI18n(createElement(ContextMenuHost, null, createElement(NewProjectDialog, { onCreate, onClose: () => {}, busy: false, error }))),
  );
  return onCreate;
}

describe("新建项目对话框", () => {
  it("只列「我的渠道」，初值是新项目默认；别的从「其他渠道」挑进来；至少留一个；一并交给宿主", async () => {
    const onCreate = show();
    const dialog = within(await screen.findByRole("dialog"));
    const pressed = (name: string) => dialog.getByRole("button", { name }).getAttribute("aria-pressed");
    const youtube = t("workspace:creator.channel.youtube");
    const bilibili = t("workspace:creator.channel.bilibili");
    const douyin = t("workspace:creator.channel.douyin");
    await waitFor(() => expect(pressed(youtube)).toBe("true"));
    expect(dialog.queryByRole("button", { name: douyin }), "不常用的不摊出来").toBeNull();
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.project.newOtherChannels") }));
    fireEvent.click(await screen.findByRole("menuitem", { name: t("workspace:creator.channelFormat.short_video") }));
    fireEvent.click(await screen.findByRole("menuitem", { name: douyin }));
    await waitFor(() => expect(pressed(douyin)).toBe("true"));
    fireEvent.click(dialog.getByRole("button", { name: bilibili }));
    fireEvent.click(dialog.getByRole("button", { name: youtube }));
    fireEvent.click(dialog.getByRole("button", { name: douyin }));
    expect(pressed(douyin), "最后一个按不掉").toBe("true");
    fireEvent.change(dialog.getByRole("textbox", { name: t("workspace:creator.project.nameLabel") }), {
      target: { value: "  第一期 " },
    });
    fireEvent.change(dialog.getByLabelText(t("workspace:creator.project.newDate")), { target: { value: "2026-11-01" } });
    fireEvent.click(dialog.getByRole("button", { name: t("workspace:creator.project.create") }));
    expect(onCreate).toHaveBeenCalledWith("第一期", { channels: ["douyin"], publish_date: "2026-11-01" });
  });

  it("没填名字不能建；宿主给的失败原因就近显示", async () => {
    const onCreate = show(vi.fn(), "已经有同名的项目了");
    const dialog = within(await screen.findByRole("dialog"));
    expect((dialog.getByRole("button", { name: t("workspace:creator.project.create") }) as HTMLButtonElement).disabled).toBe(true);
    expect(dialog.getByRole("alert").textContent).toBe("已经有同名的项目了");
    expect(onCreate).not.toHaveBeenCalled();
  });
});
