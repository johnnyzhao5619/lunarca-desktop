// @vitest-environment happy-dom
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { i18n } from "../i18n/i18n";

const t = (key: string) => i18n.t(key.replace("common:", ""), { ns: "common" });
import { AppErrorBoundary } from "./AppErrorBoundary";

afterEach(cleanup);

function Boom(): never {
  throw new RangeError("Field is not present in this state");
}

describe("根级错误兜底", () => {
  it("子树抛错时显示说明卡而不是白屏；技术细节里有错误名与消息；点重新加载即重载", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const reload = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, reload } as Location);
    render(createElement(AppErrorBoundary, null, createElement(Boom)));
    expect(screen.getByRole("alert")).toBeTruthy();
    // 文案真的加载到了（不是两边都退回裸 key 而碰巧相等）。
    expect(t("common:crash.title")).not.toBe("crash.title");
    expect(screen.getByText(t("common:crash.title"))).toBeTruthy();
    expect(screen.getByText(/RangeError: Field is not present/u)).toBeTruthy();
    fireEvent.click(screen.getByText(t("common:crash.reload")));
    expect(reload).toHaveBeenCalled();
  });

  it("没出错时原样渲染子树", () => {
    render(createElement(AppErrorBoundary, null, createElement("p", null, "正常内容")));
    expect(screen.getByText("正常内容")).toBeTruthy();
  });
});
