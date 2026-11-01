// @vitest-environment happy-dom
/**
 * 活动栏布局随哪些设置重读（PRD-07-003 / ADR-165 ③）：布局本身变了要重读；一方应用启用 /
 * 停用也要重读——未启用应用的空间由 Rust 整格不下发，只听布局键的话，启用了「创作」
 * 活动栏上也不会出现它（真机 2026-10-19）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { useRailLayout } from "./use-rail-layout";

function Probe() {
  useRailLayout();
  return null;
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("workspaceRailLayout", { status: "ok", data: [] });
});
afterEach(cleanup);

describe("活动栏布局的重读时机", () => {
  it("布局键与应用启用状态变了都重读，别的设置不重读", async () => {
    render(<Probe />);
    const load = commandMock("workspaceRailLayout");
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1));

    emitEvent("settingsChanged", { key: "apps.enabled" });
    await waitFor(() => expect(load).toHaveBeenCalledTimes(2));

    emitEvent("settingsChanged", { key: "workspace.rail_layout" });
    await waitFor(() => expect(load).toHaveBeenCalledTimes(3));

    emitEvent("settingsChanged", { key: "appearance.theme_mode" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(load).toHaveBeenCalledTimes(3);
  });
});
