// @vitest-environment happy-dom
/**
 * 知识笔记徽标的**渲染行为**守护（R1.2 / R1.3）。
 *
 * 与同目录的 `distill-incomplete.test.ts` 分工不同：那一条读源码，钉的是"实现有没有走
 * 那条路"；这一条真渲染组件，钉的是**用户到底看得到什么**。两者都需要——现网这两个缺陷
 * 的形状恰恰是"实现里有、界面上看不见"：
 *
 * ① 预检缺口的三个数只进了主按钮的 `title=`，悬停才看得到（D7′「不足当场说清缺口」
 *    退化成了「悬停才说得清」）；
 * ② 触顶三档被压成一句「内容不完整，重新整理」，而 `caller` 那一档重新整理是确定性死循环。
 */

import { createElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";

import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import type { DistillStateDto } from "../../ipc/bindings";

/** Rust 真会发出来的那一串（稳定机器前缀 + Fluent 说明，三个数由参数代入）。 */
const QUOTA_SHORT =
  "DISTILL_QUOTA_SHORT: 服务端余量不够跑完这一篇。整理它约需 9000 个输出 token，" +
  "当前还剩约 1000 个，差约 8000 个。可先补充额度，或在「模型与算力」里改用本机模型。";

const distillStatus = vi.fn();
const distillNotes = vi.fn();

vi.mock("../../ipc/bindings", () => ({
  commands: {
    distillStatus: (path: string) => distillStatus(path),
    distillNotes: (...args: unknown[]) => distillNotes(...args),
    distillTemplateOptions: () => Promise.resolve({ status: "error", error: "unused" }),
  },
  events: {
    vaultChanged: { listen: () => Promise.resolve(() => undefined) },
  },
}));
vi.mock("../processing/use-processing-queue", () => ({
  useProcessingQueueSelector: <T,>(select: (state: { items: never[]; loaded: boolean }) => T) =>
    select({ items: [], loaded: true }),
}));

// 之后再 import 组件，让上面的 mock 先就位。
const { DistillBadge } = await import("./DistillBadge");
const { ConfirmHost } = await import("../../components/ConfirmHost");

const i18n: I18nInstance = createInstance();
await i18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: "workspace",
  resources: { "zh-CN": { workspace: zhWorkspace } },
  interpolation: { escapeValue: false },
});

function stateWith(overrides: Partial<DistillStateDto>): DistillStateDto {
  return {
    eligible: true,
    source_material: true,
    note_path: null,
    stale: false,
    edited: false,
    output_truncated: null,
    template: null,
    ...overrides,
  };
}

function mount(state: DistillStateDto, onOpenNote = vi.fn()) {
  distillStatus.mockResolvedValue({ status: "ok", data: state });
  const { container } = render(
    createElement(
      I18nextProvider,
      { i18n },
      // 徽标开始整理前要过任务前预估那道门（PRD-13-040 ②），门用的是全应用唯一的确认宿主。
      createElement(
        ConfirmHost,
        null,
        createElement(DistillBadge, { notePath: "Recordings/会议.md", onOpenNote }),
      ),
    ),
  );
  return { container, onOpenNote };
}

/** 徽标主按钮渲染完这一档之后的可见文案与悬停说明。 */
async function badgeCopy(outputTruncated: string) {
  cleanup();
  const { container } = mount(
    stateWith({ note_path: "Notes/会议 · 纪要.md", output_truncated: outputTruncated }),
  );
  const button = await waitFor(() => {
    const found = container.querySelector(".luna-distill-badge__btn");
    if (!found?.textContent?.includes("不完整")) {
      throw new Error("这一档的状态还没渲染出来");
    }
    return found as HTMLElement;
  });
  return { text: button.textContent ?? "", title: button.getAttribute("title") ?? "" };
}

beforeEach(() => {
  distillStatus.mockReset();
  distillNotes.mockReset();
});

describe("预检缺口在界面上看得见（R1.3 / D7′）", () => {
  it("需要多少 / 还剩多少 / 差多少三个数进**可见文本**，不是只进 title", async () => {
    // Rust 真会回的形状：命令本身成功，这一篇被**逐篇挡下**并带着理由回来
    // （`distill_notes` 的 `rejected`）——理由消失在一句"操作失败"里正是要拦的那件事。
    distillNotes.mockResolvedValue({
      status: "ok",
      data: { queued: 0, rejected: [{ note_rel: "Recordings/会议.md", reason: QUOTA_SHORT }] },
    });
    mount(stateWith({}));

    const button = await screen.findByRole("button", { name: /生成知识笔记/ });
    button.click();

    const status = await screen.findByRole("status");
    for (const number of ["9000", "1000", "8000"]) {
      expect(
        status.textContent,
        `缺口少了 ${number}——只说「整理失败」的话，用户既不知道该补多少，也不知道该不该改用本机模型`,
      ).toContain(number);
    }
    // 机器前缀是给调用方和日志的，不该摆在用户眼前。
    expect(status.textContent).not.toContain("DISTILL_QUOTA_SHORT");
  });
});

describe("触顶三档各说各的（R1.2 / ADR-089 ⑧）", () => {
  it("三档的措辞两两不同——压成一句「被截断了」，猜错一次就是把人引去改一个改了也没用的设置", async () => {
    const caller = await badgeCopy("caller");
    const upstream = await badgeCopy("upstream");
    const absolute = await badgeCopy("absolute");

    for (const [a, b] of [
      [caller, upstream],
      [caller, absolute],
      [upstream, absolute],
    ]) {
      expect(a.text, "两档共用了同一句话").not.toBe(b.text);
      expect(a.title, "两档共用了同一句说明").not.toBe(b.title);
    }
  });

  it("`caller` 档不许开出「重新整理」这个动作：那道上限是编译期常量、切分确定性，重跑必然停在同一处", async () => {
    const caller = await badgeCopy("caller");
    const callerCopy = `${caller.text} ${caller.title}`;
    expect(callerCopy, "把用户支去重跑一遍分钟级的生成，而结果必然一模一样").not.toMatch(
      /重新整理/,
    );
    // 它得说得出真正做得到的那件事：把这一篇拆短。
    expect(callerCopy, "既不让重跑、又不说该干什么，等于只说了「坏了」").toMatch(/拆/);

    // 另外两档换一次候选可能就过去了，可以引导重试。
    const upstream = await badgeCopy("upstream");
    expect(`${upstream.text} ${upstream.title}`).toMatch(/再试|再整理/);
  });

  it("`caller` 档的主动作是「打开」而不是重跑——那是一个确定性死循环", async () => {
    const onOpenNote = vi.fn();
    const { container } = mount(
      stateWith({ note_path: "Notes/会议 · 纪要.md", output_truncated: "caller" }),
      onOpenNote,
    );
    const button = await waitFor(() => {
      const found = container.querySelector(".luna-distill-badge__btn") as HTMLElement | null;
      if (!found?.textContent?.includes("不完整")) throw new Error("状态还没到");
      return found;
    });
    button.click();
    await waitFor(() => expect(onOpenNote).toHaveBeenCalledWith("Notes/会议 · 纪要.md"));
    expect(distillNotes, "caller 档又去重跑了一遍").not.toHaveBeenCalled();
  });
});
