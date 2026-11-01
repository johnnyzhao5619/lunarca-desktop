// @vitest-environment happy-dom
/**
 * 库外改名提示条（PRD-06-030 ⑤）：只提示不自动改；「修复」走保存屏障后调 Rust，外层据此映射
 * 标签；「忽略」只撤提示；批量外部变动合成一句。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ExternalRenameBar } from "./ExternalRenameBar";

const flush = () => act(async () => {});
const one = { from: "甲/目标.md", to: "乙/改名了.md", notes: 2, links: 3 };

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("ExternalRenameBar", () => {
  it("说清是哪一篇、几处链接；修复后交给外层映射标签", async () => {
    setCommandReply("repairExternalRenames", { status: "ok", data: { notes: 2, links: 3 } });
    const onRepaired = vi.fn();
    const onStatus = vi.fn();
    render(
      withI18n(
        <ExternalRenameBar
          prompt={{ renames: [one], batch: false, changes: 2 }}
          onRepaired={onRepaired}
          onStatus={onStatus}
        />,
      ),
    );
    expect(screen.getByRole("status").textContent).toContain("「目标」在外部被改名为「改名了」，有 3 处链接仍指向旧名");
    expect(commandMock("repairExternalRenames")).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "修复" }));
    await flush();
    expect(commandMock("repairExternalRenames")).toHaveBeenCalledWith([{ from: one.from, to: one.to }]);
    expect(onRepaired).toHaveBeenCalledWith([one]);
    expect(onStatus).toHaveBeenCalledWith("已更新 2 篇笔记中的 3 处链接");
  });

  it("批量外部变动合成一句，忽略只撤提示", () => {
    render(
      withI18n(
        <ExternalRenameBar
          prompt={{ renames: [one, { ...one, from: "a.md", to: "b.md", links: 1 }], batch: true, changes: 140 }}
          onRepaired={() => {}}
          onStatus={() => {}}
        />,
      ),
    );
    expect(screen.getByRole("status").textContent).toContain("检测到批量外部变动（140 处），其中 2 篇改名后有 4 处链接仍指向旧名");
    fireEvent.click(screen.getByRole("button", { name: "忽略" }));
    expect(commandMock("dismissExternalRenames")).toHaveBeenCalledTimes(1);
    expect(commandMock("repairExternalRenames")).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "全部修复" })).toBeTruthy();
  });
});
