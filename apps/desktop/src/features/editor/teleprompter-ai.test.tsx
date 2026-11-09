// @vitest-environment happy-dom
/**
 * 提词器的 AI 口播动作（PRD-20-016，ADR-176 ⑧）：「提词器」应用生效时，任何笔记的选区菜单里都有
 * 「提词器 ▸」；结果按逐条差异审阅，「压到 N 秒」在结果上写清约几秒、超了多出几秒。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { FloatingMenuAction } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { FloatingMenu } from "./FloatingMenu";
import { AiResultPanel } from "./AiResultPanel";
import type { FloatingMenuResult } from "./use-floating-menu";

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("computeStatus", { status: "ok", data: { routes: [] } });
  setCommandReply("floatingPresetsList", { status: "ok", data: [] });
  teleprompterApp(true);
});
afterEach(cleanup);

function teleprompterApp(enabled: boolean) {
  setCommandReply("appsList", [
    {
      id: "teleprompter",
      enabled,
      space: "teleprompter",
      note_types: [],
      permissions: [],
      contributions: [],
    },
  ]);
}

function menu(scope: "selection" | "doc") {
  return render(
    withI18n(
      <FloatingMenu
        aiShortcut="cmd_or_ctrl+j"
        scope={scope}
        selectedText="今天我们来聊一聊怎么把一段书面的文字改成好念的口播稿。"
        context={null}
        noteType="note"
        anchorBox={{ top: 0, left: 0, bottom: 10, right: 10 }}
        editorSize={{ width: 800, height: 600 }}
        contextEnabled={false}
        onToggleContext={() => {}}
        onLand={() => {}}
        onClose={() => {}}
      />,
    ),
  );
}

function done(action: FloatingMenuAction, output: string): FloatingMenuResult {
  return {
    status: "done",
    output,
    message: null,
    action,
    truncatedChars: null,
    meta: null,
    chunkProgress: null,
  };
}

function panel(result: FloatingMenuResult, fitTarget: { seconds: number; estimate: () => Promise<number | null> } | null) {
  return render(
    withI18n(
      <AiResultPanel scope="selection" result={result} originalText="今天我们来聊一聊口播稿。" candidateIndex={0} candidateCount={1}
        onSelectCandidate={() => {}} routeLabel={null} acceptShortcut="⌘↵" onStop={() => {}} onRetry={() => {}}
        onReject={() => {}} onCopy={() => {}} onSaveNote={() => {}} onLand={() => {}} onFollowUp={() => {}}
        onSavePreset={null} onExplain={async () => ""} onBackground={null} fitTarget={fitTarget} />,
    ),
  );
}

describe("提词器口播动作", () => {
  it("普通笔记的选区菜单里就有「提词器」；整篇作用域没有", async () => {
    const selection = menu("selection");
    await waitFor(() =>
      expect(selection.container.textContent).toContain(t("editor:floating.action.prompter")),
    );
    cleanup();
    const doc = menu("doc");
    await waitFor(() => expect(doc.container.textContent).toContain(t("editor:floating.action.summarize")));
    expect(doc.container.textContent).not.toContain(t("editor:floating.action.prompter"));
  });

  it("「提词器」应用停用时没有这一组（停用收回编辑器动作，PRD-20-001）", async () => {
    teleprompterApp(false);
    const selection = menu("selection");
    await waitFor(() =>
      expect(selection.container.textContent).toContain(t("editor:floating.action.optimize")),
    );
    expect(selection.container.textContent).not.toContain(t("editor:floating.action.prompter"));
  });

  it.each(["prompter_spoken", "prompter_fit", "prompter_cues"] as const)(
    "%s 的结果按逐条差异审阅，确认才替换",
    (action) => {
      const { container } = panel(done(action, "今天 // 聊聊**口播稿**。"), null);
      expect(container.querySelector(".luna-floating-menu__diff")).not.toBeNull();
    },
  );

  it("压到 60 秒：超了写清多出几秒，不替你删", async () => {
    const { container } = panel(done("prompter_fit", "今天聊聊口播稿。"), {
      seconds: 60,
      estimate: async () => 72,
    });
    await waitFor(() =>
      expect(container.textContent).toContain(
        t("editor:floating.prompter.fit_over", { secs: 72, target: 60, over: 12 }),
      ),
    );
  });

  it("压到 60 秒：没超时只写约几秒", async () => {
    const { container } = panel(done("prompter_fit", "今天聊聊口播稿。"), {
      seconds: 60,
      estimate: async () => 55,
    });
    await waitFor(() =>
      expect(container.textContent).toContain(t("editor:floating.prompter.fit_ok", { secs: 55, target: 60 })),
    );
    expect(container.querySelector(".luna-floating-menu__notice")).toBeNull();
  });
});
