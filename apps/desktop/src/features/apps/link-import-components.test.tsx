// @vitest-environment happy-dom
/**
 * 「从链接导入」组件面板的内容分层（原设置「外部工具」页，ADR-167 ⑤ 起住在应用详情页）：可用的
 * 工具不说缺失；检测证实缺失才给影响与安装入口；检测失败不下缺失结论、不按未知计划安装。
 */
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { LinkImportComponents } from "./LinkImportComponents";

const ok = (data: unknown) => ({ status: "ok", data });
const READY = {
  ready: true, tool_path: "/tools/yt-dlp", version: "2026.10.01",
  ffmpeg: true, ffmpeg_path: "/tools/ffmpeg", strict_local: false,
  probe_timed_out: false, managed_version: null, using_managed: false,
  js_runtime: "Node", js_runtime_path: "/tools/node", using_managed_js: false,
};

async function tools(reply: unknown) {
  setCommandReply("fetchToolStatus", reply);
  setCommandReply("fetchInstallPlan", { components: [], install_dir: "/tools" });
  setCommandReply("listSettings", ok([]));
  setCommandReply("externalToolGuides", [{ tool: "yt_dlp", manager: "Homebrew", command: "brew install yt-dlp", docs_url: "https://github.com/yt-dlp/yt-dlp#installation" }]);
  render(withI18n(<LinkImportComponents advancedMode={false} />));
  await act(async () => {});
}

afterEach(() => { cleanup(); resetBindingsDouble(); });

describe("从链接导入的组件面板", () => {
  it("已可用工具不显示缺失警告或安装指南", async () => {
    await tools(ok(READY));
    expect(screen.queryByText("未安装时视频最高约 720p。")).toBeNull();
    expect(screen.queryByText("缺失时下载可能被站点拒绝。")).toBeNull();
    expect(screen.queryByText("自行安装…")).toBeNull();
  });

  it("检测证实缺失时显示影响和具名安装入口，不执行安装", async () => {
    await tools(ok({ ...READY, ready: false, tool_path: null, version: null, ffmpeg: false,
      ffmpeg_path: null, js_runtime: null, js_runtime_path: null }));
    expect(screen.getByText("未安装时视频最高约 720p。")).toBeTruthy();
    expect(screen.getByText("缺失时下载可能被站点拒绝。")).toBeTruthy();
    expect(screen.getByText("自行安装…").closest("details")?.open).toBe(false);
    expect(screen.getByText("brew install yt-dlp").closest("details")).toBeTruthy();
    expect(commandMock("installFetchTool")).not.toHaveBeenCalled();
  });

  it("检测失败不能显示缺失结论，也不能按未知计划直接安装", async () => {
    await tools({ status: "error", error: "FETCH_TOOL_IO" });
    expect(screen.getAllByText("暂不可获取")).toHaveLength(3);
    expect(screen.queryByText("未安装时视频最高约 720p。")).toBeNull();
    expect(screen.queryByText("缺失时下载可能被站点拒绝。")).toBeNull();
    expect(screen.queryByRole("button", { name: "安装" })).toBeNull();
    expect(screen.getByRole("button", { name: "重新检测" })).toBeTruthy();
  });

});
