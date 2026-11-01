// @vitest-environment happy-dom
/**
 * 视频脚本的改稿动作（PRD-19-013/014）：只有脚本笔记的选区菜单里有「脚本 ▸」；
 * 四个动作的结果按逐条差异审阅（与校对同一面），确认才替换。
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
import type { NoteTypeDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { FloatingMenu } from "./FloatingMenu";
import { AiResultPanel } from "./AiResultPanel";
import type { FloatingMenuResult } from "./use-floating-menu";

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("computeStatus", { status: "ok", data: { routes: [] } });
  setCommandReply("floatingPresetsList", { status: "ok", data: [] });
  creatorApp(true);
});

/** 「创作」应用的启用状态（脚本动作归它，ADR-165 ④）。 */
function creatorApp(enabled: boolean) {
  setCommandReply("appsList", [
    {
      id: "creator",
      enabled,
      space: "creator",
      note_types: ["video-project", "script", "swipe"],
      permissions: [],
      contributions: [],
    },
  ]);
}
afterEach(cleanup);

function menu(noteType: NoteTypeDto | null) {
  return render(
    withI18n(
      <FloatingMenu
        aiShortcut="cmd_or_ctrl+j"
        scope="selection"
        selectedText="大家好，那个，今天我们聊一聊脚本。"
        context={null}
        noteType={noteType}
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

describe("脚本改稿动作", () => {
  it("脚本笔记的选区菜单里有「脚本」，普通笔记没有", async () => {
    const script = menu("script");
    await waitFor(() =>
      expect(script.container.textContent).toContain(t("editor:floating.action.script")),
    );
    cleanup();
    expect(menu("note").container.textContent).not.toContain(t("editor:floating.action.script"));
  });

  it("「创作」应用停用时脚本笔记也没有「脚本」（停用收回全部入口，ADR-165 ④）", async () => {
    creatorApp(false);
    const script = menu("script");
    // 等清单取回（取回前本就不露），再确认它仍然不在。
    await waitFor(() => expect(script.container.textContent).toContain(t("editor:floating.action.optimize")));
    expect(script.container.textContent).not.toContain(t("editor:floating.action.script"));
  });

  it("改稿结果按逐条差异审阅，而不是整段替换", () => {
    const done: FloatingMenuResult = {
      status: "done",
      output: "大家好，今天聊脚本。",
      message: null,
      action: "script_tighten",
      truncatedChars: null,
      meta: null,
      chunkProgress: null,
    };
    const { container } = render(
      withI18n(
        <AiResultPanel scope="selection" result={done} originalText="大家好，那个，今天我们聊一聊脚本。" candidateIndex={0} candidateCount={1}
          onSelectCandidate={() => {}} routeLabel={null} acceptShortcut="⌘↵" onStop={() => {}} onRetry={() => {}}
          onReject={() => {}} onCopy={() => {}} onSaveNote={() => {}} onLand={() => {}} onFollowUp={() => {}}
          onSavePreset={null} onExplain={async () => ""} onBackground={null} />,
      ),
    );
    expect(container.querySelector(".luna-floating-menu__diff")).not.toBeNull();
  });
});
