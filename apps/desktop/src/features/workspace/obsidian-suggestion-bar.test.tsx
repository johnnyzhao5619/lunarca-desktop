// @vitest-environment happy-dom
/**
 * 「沿用 Obsidian 的设置？」（PRD-06-064，V4c）：把不同之处说出来；沿用 / 不用都交给 Rust
 * 记下并收起，沿用后报一声。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ObsidianSuggestionBar } from "./ObsidianSuggestionBar";

const flush = () => act(async () => {});
const suggestion = {
  attachment_location: "note_subfolder",
  attachment_folder: "附件",
  exclusions: ["归档", "模板"],
  skipped_patterns: 1,
};

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("ObsidianSuggestionBar", () => {
  it("说清要改的附件位置与排除项，沿用后收起并报一声", async () => {
    setCommandReply("acceptObsidianSuggestion", { status: "ok", data: null });
    const onSettled = vi.fn();
    const onStatus = vi.fn();
    render(withI18n(<ObsidianSuggestionBar suggestion={suggestion} onSettled={onSettled} onStatus={onStatus} />));
    const text = screen.getByRole("status").textContent ?? "";
    expect(text).toContain("这个库也在用 Obsidian");
    expect(text).toContain("附件放在笔记所在文件夹下的「附件」");
    expect(text).toContain("排除 2 处");
    expect(text).toContain("另有 1 条正则写法的排除无法沿用");
    fireEvent.click(screen.getByRole("button", { name: "沿用" }));
    await flush();
    expect(commandMock("acceptObsidianSuggestion")).toHaveBeenCalledTimes(1);
    expect(onStatus).toHaveBeenCalledWith("已沿用 Obsidian 的设置");
    expect(onSettled).toHaveBeenCalledTimes(1);
  });

  it("不用：只记下问过了，不改任何设置", async () => {
    setCommandReply("dismissObsidianSuggestion", { status: "ok", data: null });
    const onSettled = vi.fn();
    render(
      withI18n(
        <ObsidianSuggestionBar
          suggestion={{ ...suggestion, attachment_location: null, attachment_folder: null, skipped_patterns: 0 }}
          onSettled={onSettled}
          onStatus={() => {}}
        />,
      ),
    );
    expect(screen.getByRole("status").textContent).not.toContain("附件放在");
    fireEvent.click(screen.getByRole("button", { name: "不用" }));
    await flush();
    expect(commandMock("dismissObsidianSuggestion")).toHaveBeenCalledTimes(1);
    expect(commandMock("acceptObsidianSuggestion")).not.toHaveBeenCalled();
    expect(onSettled).toHaveBeenCalledTimes(1);
  });
});
