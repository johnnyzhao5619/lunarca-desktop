// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { FloatingMenu, type TableAiTask } from "./FloatingMenu";
import { AiResultPanel } from "./AiResultPanel";
import type { FloatingMenuResult } from "./use-floating-menu";

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("computeStatus", { status: "ok", data: { routes: [] } });
  setCommandReply("floatingPresetsList", { status: "ok", data: [] });
  // 选区菜单按已启用的应用摆放专属动作组（提词器 / 创作）：表格用例里一个都不开。
  setCommandReply("appsList", []);
});
afterEach(cleanup);

const TABLE = "| 产品 | 卖点 |\n| --- | --- |\n| 灯 |  |";

function mount(tableTask: TableAiTask) {
  return render(
    withI18n(
      <FloatingMenu
        aiShortcut="cmd_or_ctrl+j"
        scope="selection"
        selectedText={TABLE}
        context={null}
        anchorBox={{ top: 0, left: 0, bottom: 10, right: 10 }}
        editorSize={{ width: 800, height: 600 }}
        contextEnabled={false}
        onToggleContext={() => {}}
        onLand={() => {}}
        onClose={() => {}}
        tableTask={tableTask}
      />,
    ),
  );
}

describe("表格菜单打开的 AI 面板（对标方案 T6）", () => {
  it("「总结这张表」打开即以整张表为作用域提交 table_explain", async () => {
    mount({ kind: "explain" });
    await waitFor(() => expect(commandMock("submitFloatingMenu")).toHaveBeenCalled());
    const [text, action, scope] = commandMock("submitFloatingMenu").mock.calls[0];
    expect([text, action, scope]).toEqual([TABLE, "table_explain", "selection"]);
  });

  it("「填写这一列」只给指令输入，回车按列号提交 table_fill_column", async () => {
    const { container } = mount({ kind: "fill_column", column: 1, header: "卖点" });
    const input = container.querySelector<HTMLInputElement>("input")!;
    expect(input.placeholder).toContain("卖点");
    fireEvent.change(input, { target: { value: "根据产品名写一句卖点" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(commandMock("submitFloatingMenu")).toHaveBeenCalled());
    const call = commandMock("submitFloatingMenu").mock.calls[0];
    expect(call[1]).toBe("table_fill_column");
    expect(call[3]).toBe("根据产品名写一句卖点");
    expect(call[6]).toMatchObject({ table_column: 1 });
  });

  it("逐行填写的进度说「第几行」，不借用分段摘要的「第几段」", () => {
    const running = (action: FloatingMenuResult["action"]): FloatingMenuResult => ({
      status: "running", output: "", message: null, action, truncatedChars: null, meta: null,
      chunkProgress: { done: 0, total: 3 },
    });
    const panel = (result: FloatingMenuResult) => render(withI18n(
      <AiResultPanel scope="selection" result={result} originalText={TABLE} candidateIndex={0} candidateCount={1}
        onSelectCandidate={() => {}} routeLabel={null} acceptShortcut="⌘↵" onStop={() => {}} onRetry={() => {}}
        onReject={() => {}} onCopy={() => {}} onSaveNote={() => {}} onLand={() => {}} onFollowUp={() => {}}
        onSavePreset={null} onExplain={async () => ""} onBackground={null} fitTarget={null} />,
    ));
    expect(panel(running("table_fill_column")).container.textContent).toContain("正在填写第 1 / 3 行");
    cleanup();
    expect(panel(running("summarize")).container.textContent).toContain("正在摘要第 1 / 3 段");
  });
});
