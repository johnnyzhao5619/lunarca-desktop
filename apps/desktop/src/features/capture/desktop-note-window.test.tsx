// @vitest-environment happy-dom
/**
 * 桌面便签的写盘纪律（ADR-119 ④⑤）。
 *
 * 真机 2026-09-09：钉到桌面之后整篇正文被清空。根因是保存由 state 变化发起——组件
 * 挂载时正文与去抖值都是空串，`load()` 回来把内容立刻设上，而去抖值还停在空串，那一帧
 * 就把空串写进了磁盘。
 *
 * 这份用例把两条纪律钉住：
 * ① 打开一张便签不产生任何写盘；
 * ② 写回必须带基线，基线对不上就重读而不是覆盖。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    outerPosition: () => Promise.resolve({ x: 0, y: 0 }),
    innerSize: () => Promise.resolve({ width: 260, height: 220 }),
    onMoved: () => Promise.resolve(() => {}),
    onResized: () => Promise.resolve(() => {}),
    startResizeDragging: () => Promise.resolve(),
  }),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ startDragging: () => Promise.resolve() }),
}));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { DesktopNoteWindow } from "./DesktopNoteWindow";

const BODY = "买牛奶\n买梨\n";

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("desktopNoteContent", {
    status: "ok",
    data: { path: "QuickNotes/买菜.md", title: "买菜", body: BODY, color: null },
  });
  setCommandReply("desktopNoteWriteBody", { status: "ok", data: true });
});

afterEach(cleanup);

function mount() {
  const view = render(withI18n(<DesktopNoteWindow />));
  // 有正文时静息态是渲染面；要改字先点进去——与真机上的手势同一步。
  const area = () => {
    const rendered = view.container.querySelector<HTMLElement>(".luna-sticky__rendered");
    if (rendered !== null) fireEvent.mouseDown(rendered);
    return view.container.querySelector("textarea");
  };
  return { view, area };
}

describe("桌面便签的写盘纪律", () => {
  it("读回来之前不给可编辑的框——空框与「这篇本来就是空的」长得一样", async () => {
    // 那一刻敲的任何一个字都会以一个假的基线为起点，而屏幕上看不出任何异常。
    let release: (value: unknown) => void = () => {};
    setCommandReply("desktopNoteContent", () => new Promise((resolve) => (release = resolve)));
    const { view, area } = mount();
    expect(area(), "还没读回来就不该有输入框").toBeNull();
    expect(view.container.querySelector(".luna-sticky__loading")).not.toBeNull();

    release({
      status: "ok",
      data: { path: "QuickNotes/买菜.md", title: "买菜", body: BODY, color: null },
    });
    await waitFor(() => expect(area()?.value).toBe(BODY));
  });

  it("打开一张便签不产生任何写盘——正文被清空的那个 bug 就长在这一步", async () => {
    const { area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    // 等过去抖窗口：由 state 变化发起的保存会在这里冒出来。
    await new Promise((resolve) => setTimeout(resolve, 900));
    expect(
      commandMock("desktopNoteWriteBody"),
      "只是打开看一眼就写盘，等于让每一次钉住都有机会覆盖用户的正文",
    ).not.toHaveBeenCalled();
  });

  it("用户敲字才写，且必须带上它以为磁盘上是什么", async () => {
    const { area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    fireEvent.change(area()!, { target: { value: `${BODY}买米\n` } });
    await waitFor(() =>
      expect(commandMock("desktopNoteWriteBody")).toHaveBeenCalledWith(BODY, `${BODY}买米\n`),
    );
  });

  it("基线对不上时保留本地内容并明确提示，不覆盖别处的改动", async () => {
    setCommandReply("desktopNoteWriteBody", { status: "ok", data: false });
    const { view, area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    fireEvent.change(area()!, { target: { value: "我在便签里敲的" } });
    await waitFor(() => expect(commandMock("desktopNoteWriteBody")).toHaveBeenCalled());
    expect(area()!.value).toBe("我在便签里敲的");
    await waitFor(() => expect(view.getByRole("alert")).toBeTruthy());
  });

  it("冲突之后停掉自动保存，并给两条出路：另存为新速记 / 改用别处的版本", async () => {
    setCommandReply("desktopNoteWriteBody", { status: "ok", data: false });
    setCommandReply("createNoteFromText", {
      status: "ok",
      data: { path: "QuickNotes/新.md", content: "", meta: null },
    });
    const { view, area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    fireEvent.change(area()!, { target: { value: "便签里这段" } });
    await waitFor(() => expect(view.getByText(t("workspace:desktopNote.saveAsNew"))).toBeTruthy());
    const writes = commandMock("desktopNoteWriteBody").mock.calls.length;
    // 再敲字：基线已对不上，不再发注定被拒的写入。
    fireEvent.change(area()!, { target: { value: "便签里这段，又加一句" } });
    await new Promise((resolve) => setTimeout(resolve, 700));
    expect(commandMock("desktopNoteWriteBody").mock.calls.length).toBe(writes);

    const loadsBefore = commandMock("desktopNoteContent").mock.calls.length;
    fireEvent.click(view.getByText(t("workspace:desktopNote.saveAsNew")));
    await waitFor(() =>
      expect(commandMock("createNoteFromText")).toHaveBeenCalledWith("便签里这段，又加一句", "drafts"),
    );
    // 另存之后读回磁盘版本：两边都在，便签显示别处的那一份。
    await waitFor(() =>
      expect(commandMock("desktopNoteContent").mock.calls.length).toBeGreaterThan(loadsBefore),
    );
    await waitFor(() => expect(area()!.value).toBe(BODY));
    expect(view.queryByRole("alert")).toBeNull();
  });

  it("读失败不停在「载入中」：说原因并给重试", async () => {
    setCommandReply("desktopNoteContent", { status: "error", error: "VAULT_IO: 读不出" });
    const { view, area } = mount();
    await waitFor(() => expect(view.getByText(t("workspace:desktopNote.loadFailed"))).toBeTruthy());
    expect(area()).toBeNull();
    setCommandReply("desktopNoteContent", {
      status: "ok",
      data: { path: "QuickNotes/买菜.md", title: "买菜", body: BODY, color: null },
    });
    fireEvent.click(view.getByText(t("workspace:desktopNote.retryLoad")));
    await waitFor(() => expect(area()?.value).toBe(BODY));
  });

  it("外部改动在本地没有未落盘编辑时才接管", async () => {
    const { area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));

    // 本地有未落盘编辑：外部内容不许顶掉用户正在敲的字。
    fireEvent.change(area()!, { target: { value: "半句还没敲完" } });
    setCommandReply("desktopNoteContent", {
      status: "ok",
      data: { path: "QuickNotes/买菜.md", title: "买菜", body: "别处改的", color: null },
    });
    // 外部改动走 `vault://changed`（复用桥是另一回事：那说明这扇窗换了一篇，
    // 基线本就该作废）。
    emitEvent("vaultChanged", { rel_path: "QuickNotes/买菜.md", deleted: false });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(area()!.value, "外部改动不许顶掉用户正在敲的字").toBe("半句还没敲完");
  });

  describe("有未落盘编辑时别处改了同一篇", () => {
    async function collide() {
      const ctx = mount();
      await waitFor(() => expect(ctx.area()!.value).toBe(BODY));
      fireEvent.change(ctx.area()!, { target: { value: "便签里敲的" } });
      setCommandReply("desktopNoteContent", {
        status: "ok",
        data: { path: "QuickNotes/买菜.md", title: "买菜", body: "别处改的", color: null },
      });
      emitEvent("vaultChanged", { rel_path: "QuickNotes/买菜.md", deleted: false });
      return ctx;
    }

    it("当场亮冲突、不写盘——别处的改动不许被便签这段静默覆盖", async () => {
      // 第一版把基线推到了别处的新版本，去抖之后那次保存基线对得上、照写不误。
      const { view, area } = await collide();
      await waitFor(() => expect(view.getByRole("alert").textContent).toContain(t("workspace:desktopNote.saveConflict")));
      await new Promise((resolve) => setTimeout(resolve, 900));
      expect(commandMock("desktopNoteWriteBody"), "冲突未解决前不许写盘").not.toHaveBeenCalled();
      expect(area()!.value).toBe("便签里敲的");
    });

    it("「改用别处的版本」读回磁盘那一版", async () => {
      const { view, area } = await collide();
      fireEvent.click(await view.findByRole("button", { name: t("workspace:desktopNote.useDiskVersion") }));
      await waitFor(() => expect(area()!.value).toBe("别处改的"));
      expect(view.queryByRole("alert")).toBeNull();
    });

    it("「另存为新速记」先把便签这段存成新的一条，再读回磁盘那一版——两边都在", async () => {
      setCommandReply("createNoteFromText", { status: "ok", data: "Drafts/便签里敲的.md" });
      const { view, area } = await collide();
      fireEvent.click(await view.findByRole("button", { name: t("workspace:desktopNote.saveAsNew") }));
      await waitFor(() => expect(area()!.value).toBe("别处改的"));
      expect(commandMock("createNoteFromText")).toHaveBeenCalledWith("便签里敲的", "drafts");
      expect(commandMock("desktopNoteWriteBody")).not.toHaveBeenCalled();
    });
  });

  it("自己那次写盘的回声不算冲突", async () => {
    const { view, area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    setCommandReply("desktopNoteWriteBody", { status: "ok", data: true });
    fireEvent.change(area()!, { target: { value: "便签里敲的" } });
    // 读回来的正是便签自己写的那段（回声先于写入返回、或写入刚返回）。
    setCommandReply("desktopNoteContent", {
      status: "ok",
      data: { path: "QuickNotes/买菜.md", title: "买菜", body: "便签里敲的", color: null },
    });
    emitEvent("vaultChanged", { rel_path: "QuickNotes/买菜.md", deleted: false });
    await new Promise((resolve) => setTimeout(resolve, 900));
    expect(view.queryByRole("alert")).toBeNull();
    expect(area()!.value).toBe("便签里敲的");
  });

  it("主窗保存也要跟着变——应用内的保存走 `note://written`，不发 `vault://changed`", async () => {
    // 真机 2026-09-09 复验第 4 条：主区改完并显示「已保存」，便签仍停在上一版。根因是
    // 只订了 `vault://changed`，而 `write_note` 刻意不发那条（自写回声抑制会吞掉它）。
    const { area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    setCommandReply("desktopNoteContent", {
      status: "ok",
      data: { path: "QuickNotes/买菜.md", title: "买菜", body: "主窗写的", color: null },
    });
    emitEvent("noteWritten", {
      path: "QuickNotes/买菜.md",
      content: "---\nluna: {}\n---\n主窗写的",
      origin: "main",
    });
    await waitFor(() => expect(area()!.value).toBe("主窗写的"));
  });

  it("别的笔记被写不该惊动这一张便签", async () => {
    const { area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    const loads = commandMock("desktopNoteContent").mock.calls.length;
    emitEvent("noteWritten", { path: "QuickNotes/别的.md", content: "x", origin: "main" });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(commandMock("desktopNoteContent").mock.calls.length).toBe(loads);
  });

  it("换颜色：就地变色 + 落一条命令，正文一个字不动", async () => {
    // 配色（ADR-119 ②）的四个色档在样式里躺了一轮才有入口。选中的那一个套描边，
    // 而"点了要立刻变色"是这类小窗的基本预期——等一次 IPC 往返会被读成没点上。
    const { view, area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    const swatch = (key: string) =>
      view.container.querySelector<HTMLElement>(`[data-swatch="${key}"]`)!;

    expect(swatch("default").getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(swatch("green"));

    expect(commandMock("desktopNoteSetColor")).toHaveBeenCalledWith("green");
    await waitFor(() =>
      expect(view.container.querySelector(".luna-sticky")!.getAttribute("data-color")).toBe(
        "green",
      ),
    );
    expect(swatch("green").getAttribute("aria-pressed")).toBe("true");
    expect(swatch("default").getAttribute("aria-pressed")).toBe("false");
    // 改的是外观，不是内容。
    expect(commandMock("desktopNoteWriteBody")).not.toHaveBeenCalled();
    expect(area()!.value).toBe(BODY);
  });

  it("关掉便签走的是取消钉住，且先把排着的那次保存落下去", async () => {
    // ✕ = 取消钉住（ADR-119 ①）：桌面上没有"暂时藏起来的便签"这个状态。
    const { view, area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    fireEvent.click(view.getByLabelText("取消钉住"));
    await waitFor(() =>
      expect(commandMock("desktopNoteUnpin")).toHaveBeenCalledWith("QuickNotes/买菜.md"),
    );
  });

  it("关闭前保存失败时不取消钉住，并保留正文供用户重试", async () => {
    setCommandReply("desktopNoteWriteBody", { status: "error", error: "VAULT_IO: 磁盘已满" });
    const { view, area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    fireEvent.change(area()!, { target: { value: `${BODY}最后一句` } });
    fireEvent.click(view.getByLabelText("取消钉住"));

    await waitFor(() => expect(view.getByRole("alert")).toBeTruthy());
    expect(commandMock("desktopNoteUnpin")).not.toHaveBeenCalled();
    expect(area()!.value).toBe(`${BODY}最后一句`);
  });

  it("本地没有未落盘编辑时，外部改动直接接管", async () => {
    const { area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    setCommandReply("desktopNoteContent", {
      status: "ok",
      data: { path: "QuickNotes/买菜.md", title: "买菜", body: "别处改的", color: null },
    });
    emitEvent("vaultChanged", { rel_path: "QuickNotes/买菜.md", deleted: false });
    await waitFor(() => expect(area()!.value).toBe("别处改的"));
  });
});

describe("桌面便签的呈现（ADR-137）", () => {
  it("保存状态点：敲字即「待存」，落盘后回到「已存」", async () => {
    const { view, area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    const dot = () => view.container.querySelector(".luna-sticky__save-dot");
    expect(dot()?.getAttribute("data-state")).toBe("saved");
    fireEvent.change(area()!, { target: { value: `${BODY}买米\n` } });
    expect(dot()?.getAttribute("data-state")).toBe("pending");
    await waitFor(() => expect(commandMock("desktopNoteWriteBody")).toHaveBeenCalled());
    await waitFor(() => expect(dot()?.getAttribute("data-state")).toBe("saved"));
  });

  it("Esc 只让正文失焦，不摘便签", async () => {
    const { area } = mount();
    await waitFor(() => expect(area()!.value).toBe(BODY));
    area()!.focus();
    expect(document.activeElement).toBe(area());
    fireEvent.keyDown(area()!, { key: "Escape" });
    expect(document.activeElement).not.toBe(area());
    expect(commandMock("desktopNoteUnpin")).not.toHaveBeenCalled();
  });
});

describe("便签正文的看态与改态", () => {
  it("不在编辑时按 Markdown 排好，不露 `##`", async () => {
    setCommandReply("desktopNoteContent", {
      status: "ok",
      data: { path: "QuickNotes/会.md", title: "会", body: "## 要点\n\n- 第一条\n", color: null },
    });
    const view = render(withI18n(<DesktopNoteWindow />));
    const rendered = await waitFor(() => {
      const node = view.container.querySelector<HTMLElement>(".luna-sticky__rendered");
      expect(node).not.toBeNull();
      return node!;
    });
    expect(rendered.querySelector("h2")?.textContent).toBe("要点");
    expect(rendered.querySelector("li")?.textContent).toBe("第一条");
    expect(rendered.textContent).not.toContain("##");
    expect(view.container.querySelector("textarea"), "看态不该同时摆一个原文框").toBeNull();
  });

  it("点一下进原文框且拿到焦点，离开就换回渲染", async () => {
    const view = render(withI18n(<DesktopNoteWindow />));
    const rendered = await waitFor(() => {
      const node = view.container.querySelector<HTMLElement>(".luna-sticky__rendered");
      expect(node).not.toBeNull();
      return node!;
    });
    // WKWebView 里点击不给焦点：mousedown 必须被拦下，由组件自己把焦点交给原文框。
    expect(fireEvent.mouseDown(rendered), "mousedown 的默认行为会把焦点收回 body").toBe(false);
    const area = view.container.querySelector("textarea");
    expect(area?.value).toBe(BODY);
    expect(document.activeElement).toBe(area);
    fireEvent.blur(area!);
    expect(view.container.querySelector("textarea")).toBeNull();
    expect(view.container.querySelector(".luna-sticky__rendered")).not.toBeNull();
  });

  it("空便签直接给原文框，敲下第一个字也不被换走", async () => {
    setCommandReply("desktopNoteContent", {
      status: "ok",
      data: { path: "QuickNotes/空.md", title: "空", body: "", color: null },
    });
    const view = render(withI18n(<DesktopNoteWindow />));
    const area = await waitFor(() => {
      const node = view.container.querySelector("textarea");
      expect(node).not.toBeNull();
      return node!;
    });
    fireEvent.focus(area);
    fireEvent.change(area, { target: { value: "#" } });
    expect(view.container.querySelector("textarea")?.value).toBe("#");
  });
});
