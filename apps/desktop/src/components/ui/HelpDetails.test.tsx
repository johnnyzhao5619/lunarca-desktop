// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { HelpDetails } from "./HelpDetails";

beforeEach(() => {
  document.documentElement.style.setProperty("--space-2", "8px");
  document.documentElement.style.setProperty("--space-3", "12px");
});
afterEach(cleanup);

const explanation = {
  storage: "Full storage path", catalog: "Catalog details",
  first: "First explanation", second: "Second explanation",
};

describe("问号上下文说明", () => {
  it("常态不堆叠正文，点击后以独立浮层显示，重复点击关闭", () => {
    const { container } = render(<HelpDetails summary="Model storage"><p>{explanation.storage}</p></HelpDetails>);
    expect(screen.queryByText("Full storage path")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Model storage" }));
    const dialog = screen.getByRole("dialog", { name: "Model storage" });
    expect(dialog.textContent).toContain("Full storage path");
    expect(container.contains(dialog)).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Model storage" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("浮层内点击与滚动保留说明，外部点击或页面滚动关闭", () => {
    render(<HelpDetails summary="Catalog"><p>{explanation.catalog}</p></HelpDetails>);
    const trigger = screen.getByRole("button");
    fireEvent.click(trigger);
    fireEvent.pointerDown(screen.getByText("Catalog details"));
    fireEvent.scroll(screen.getByRole("dialog"));
    expect(screen.queryByRole("dialog")).not.toBeNull();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(trigger);
    fireEvent.scroll(document);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("同一时间仅打开一处说明，切换不会留下旧浮层", () => {
    render(<><HelpDetails summary="First"><p>{explanation.first}</p></HelpDetails>
      <HelpDetails summary="Second"><p>{explanation.second}</p></HelpDetails></>);
    fireEvent.click(screen.getByRole("button", { name: "First" }));
    fireEvent.click(screen.getByRole("button", { name: "Second" }));
    expect(screen.queryByRole("dialog", { name: "First" })).toBeNull();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "First" }).getAttribute("aria-expanded")).toBe("false");
  });
});
