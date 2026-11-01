// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { PathValue } from "../../components/ui/PathDetails";

const ok = (data: unknown) => ({ status: "ok", data });

afterEach(() => { cleanup(); resetBindingsDouble(); });

describe("设置内容分层", () => {
  it("复制使用完整路径，失败不报成功，路径变更后反馈不串行", async () => {
    setCommandReply("popupCopy", { status: "error", error: "CLIPBOARD_UNAVAILABLE" });
    const first = "/Users/test/Library/Application Support/Lunarca/models";
    const view = render(withI18n(<PathValue path={first} />));
    fireEvent.click(screen.getByRole("button", { name: "复制路径" }));
    await act(async () => {});
    expect(commandMock("popupCopy")).toHaveBeenCalledWith(first);
    expect(screen.getByRole("status").textContent).toContain("复制失败");
    setCommandReply("popupCopy", ok(null));
    fireEvent.click(screen.getByRole("button", { name: "复制路径" }));
    await act(async () => {});
    expect(screen.getByRole("status").textContent).toBe("已复制路径");
    view.rerender(withI18n(<PathValue path="/Volumes/Models" />));
    expect(screen.queryByRole("status")).toBeNull();
  });
});
