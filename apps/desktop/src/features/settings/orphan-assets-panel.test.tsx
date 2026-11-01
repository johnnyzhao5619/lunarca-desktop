// @vitest-environment happy-dom
/**
 * 未被引用的附件面板（PRD-06-012 ②）：只列不删；勾选、确认之后才移走，移走后重新查找，
 * 列出之后被引用上的如实说「已保留」。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { ConfirmHost } from "../../components/ConfirmHost";
import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { OrphanAssetsPanel } from "./OrphanAssetsPanel";

const ok = (data: unknown) => ({ status: "ok", data });
const flush = () => act(async () => {});

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("OrphanAssetsPanel", () => {
  it("查找之后才列；勾选并确认才移走，被引用上的说已保留", async () => {
    setCommandReply(
      "orphanAssets",
      ok({
        items: [
          { path: "笔记/assets/大图.png", bytes: 2048 },
          { path: "旧.pdf", bytes: 10 },
        ],
        total_bytes: 2058,
      }),
    );
    setCommandReply("removeOrphanAssets", ok({ removed: [], kept: ["旧.pdf"], failed: [] }));
    render(withI18n(<ConfirmHost><OrphanAssetsPanel /></ConfirmHost>));
    expect(commandMock("orphanAssets")).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "查找" }));
    await flush();
    expect(screen.getByText("大图.png")).toBeTruthy();
    expect(screen.getByText("笔记/assets")).toBeTruthy();
    const remove = screen.getByRole("button", { name: "移走所选（0 个）" }) as HTMLButtonElement;
    expect(remove.disabled).toBe(true);

    fireEvent.click(screen.getByRole("checkbox", { name: /旧\.pdf/ }));
    fireEvent.click(screen.getByRole("button", { name: "移走所选（1 个）" }));
    await flush();
    expect(commandMock("removeOrphanAssets")).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "移走" }));
    await flush();
    expect(commandMock("removeOrphanAssets")).toHaveBeenCalledWith(["旧.pdf"]);
    expect(screen.getByText(/1 个在列出之后被笔记引用，已保留/)).toBeTruthy();
    expect(commandMock("orphanAssets")).toHaveBeenCalledTimes(2);
  });
});
