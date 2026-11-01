// @vitest-environment happy-dom
/**
 * 「从链接导入」缺工具时的安装计划（PRD-05-007 2026-10-08 修订）：
 * 点击之前列出 Rust 给的计划、执行时把看过的组件交回核对；计划变了就重取并要求再确认；
 * 运行时那一半没装上时如实说出来（此前对话框把它静默吞掉）；严格本地去隐私页而不是录音页。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ConfirmHost } from "../../components/ConfirmHost";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { UrlImportDialog } from "./UrlImportDialog";

const ok = (data: unknown) => ({ status: "ok", data });
const err = (error: string) => ({ status: "error", error });
const flush = () => act(async () => {});

const MISSING = {
  ready: false,
  tool_path: null,
  version: null,
  ffmpeg: true,
  ffmpeg_path: "/opt/homebrew/bin/ffmpeg",
  strict_local: false,
  probe_timed_out: false,
  managed_version: null,
  using_managed: false,
  js_runtime: null,
  js_runtime_path: null,
  using_managed_js: false,
};
const READY = { ...MISSING, ready: true, tool_path: "/x/tools/yt-dlp", version: "2026.10.01" };
const MB = 1024 * 1024;
const component = (tool: string, disk: number) => ({
  tool,
  replaces_managed: false,
  source: `github.com/${tool}`,
  approx_download_bytes: 38 * MB,
  approx_disk_bytes: disk,
});
const BOTH = {
  components: [component("yt_dlp", 38 * MB), component("js_runtime", 77 * MB)],
  install_dir: "/x/tools",
};
const ONLY_TOOL = { components: [component("yt_dlp", 38 * MB)], install_dir: "/x/tools" };

function mount(onOpenSettings = vi.fn()) {
  render(
    withI18n(
      <ConfirmHost>
        <UrlImportDialog onClose={vi.fn()} onQueued={vi.fn()} onOpenSettings={onOpenSettings} />
      </ConfirmHost>,
    ),
  );
  return onOpenSettings;
}

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("从链接导入 · 安装计划", () => {
  it("缺工具时先列出这台机器的计划，安装时把看过的组件交回核对", async () => {
    setCommandReply("fetchToolStatus", ok(MISSING));
    setCommandReply("fetchInstallPlan", BOTH);
    setCommandReply("readClipboardText", ok(""));
    setCommandReply("listSettings", ok([]));
    setCommandReply("externalToolGuides", []);
    setCommandReply("installFetchTool", ok({ tool_version: "2026.10.01", js_runtime_version: "v2.5.0", js_runtime_error: null }));
    mount();
    await flush();

    expect(screen.getByText("需要安装下载工具")).toBeTruthy();
    expect(screen.getByText("yt-dlp（视频下载）")).toBeTruthy();
    expect(screen.getByText("Deno（部分站点取流需要）")).toBeTruthy();
    const fullPath = screen.getByText("/x/tools");
    expect(fullPath.closest("details")?.open).toBe(false);
    expect(screen.getByText("完整路径")).toBeTruthy();

    setCommandReply("fetchToolStatus", ok(READY));
    fireEvent.click(screen.getByRole("button", { name: "安装" }));
    await flush();
    expect(commandMock("installFetchTool")).toHaveBeenCalledWith(["yt_dlp", "js_runtime"]);
    expect(screen.getByText("已安装 yt-dlp 2026.10.01 和 Deno v2.5.0。")).toBeTruthy();
  });

  it("计划在两次点击之间变了：不装，重取计划并提示再看一眼", async () => {
    setCommandReply("fetchToolStatus", ok(MISSING));
    setCommandReply("fetchInstallPlan", BOTH);
    setCommandReply("readClipboardText", ok(""));
    setCommandReply("listSettings", ok([]));
    setCommandReply("externalToolGuides", []);
    setCommandReply("installFetchTool", err("FETCH_TOOL_PLAN_CHANGED"));
    mount();
    await flush();

    setCommandReply("fetchInstallPlan", ONLY_TOOL);
    fireEvent.click(screen.getByRole("button", { name: "安装" }));
    await flush();
    expect(screen.getByRole("alert").textContent).toContain("安装计划已更新");
    expect(screen.queryByText("Deno（部分站点取流需要）")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "安装" }));
    await flush();
    expect(commandMock("installFetchTool")).toHaveBeenLastCalledWith(["yt_dlp"]);
  });

  it("yt-dlp 装上而运行时没装上：门打开，同时如实说出缺的那一件", async () => {
    setCommandReply("fetchToolStatus", ok(MISSING));
    setCommandReply("fetchInstallPlan", BOTH);
    setCommandReply("readClipboardText", ok(""));
    setCommandReply("listSettings", ok([]));
    setCommandReply("externalToolGuides", []);
    setCommandReply(
      "installFetchTool",
      ok({ tool_version: "2026.10.01", js_runtime_version: null, js_runtime_error: "FETCH_TOOL_NETWORK: timeout" }),
    );
    const open = mount();
    await flush();

    setCommandReply("fetchToolStatus", ok(READY));
    fireEvent.click(screen.getByRole("button", { name: "安装" }));
    await flush();
    expect(screen.getByLabelText("视频地址")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Deno 没有装上");
    // 组件的管理在「应用 ▸ 从链接导入」（ADR-167 ⑤）：深链到应用详情，不再是设置页。
    const routes: string[] = [];
    const listener = (event: Event) => {
      routes.push(String((event as CustomEvent).detail));
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, listener);
    fireEvent.click(screen.getByRole("button", { name: "去「从链接导入」处理" }));
    window.removeEventListener(NAVIGATE_EVENT, listener);
    expect(routes).toEqual(["apps://link_import"]);
    expect(open).not.toHaveBeenCalled();
  });

  it("严格本地：去隐私页的那一项，而不是录音页", async () => {
    setCommandReply("fetchToolStatus", ok({ ...READY, ready: false, strict_local: true }));
    setCommandReply("fetchInstallPlan", ONLY_TOOL);
    setCommandReply("readClipboardText", ok(""));
    setCommandReply("listSettings", ok([]));
    setCommandReply("externalToolGuides", []);
    const open = mount();
    await flush();

    fireEvent.click(screen.getByRole("button", { name: "打开隐私设置" }));
    expect(open).toHaveBeenCalledWith("settings://privacy/privacy.strict_local");
    expect(commandMock("installFetchTool")).not.toHaveBeenCalled();
  });
});
