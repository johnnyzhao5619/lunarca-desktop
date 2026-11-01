// @vitest-environment happy-dom
/**
 * 模态/浮层行为 hook 的**时机契约**（PRD-00 §7 / TDD-07 §4.2c）。
 *
 * **本文件此前整篇是源码字符串断言**（"有没有写 `useState<T | null>(null)`"）。它证明的
 * 是"某个写法还在"，而不是"焦点真的进去了"——同样的写法换个等价形式（`useReducer`、
 * 另起变量名）就会误红，而真正的复发（依赖数组少一项、监听挂在旧元素上）它照样看不见。
 * 现在跑的是真实 DOM：挂载、按键、卸载都真的发生，断言的是 `document.activeElement`
 * 与 `onClose` 有没有被叫到。
 *
 * DOM 环境的作用域**只到本文件**（顶部 docblock），不进 `vite.config.ts` 的全局
 * `environment`：全仓约 90 个纯数据用例跑在 node 里，给它们一起套上 happy-dom 只是白付
 * 启动开销。
 */

import { createElement, useState, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

import { useDialogBehavior, type DialogBehaviorOptions } from "./use-dialog-behavior";

// `globals: false`（本仓配置）下 RTL 不会自动注册清理——不手动清，上一个用例的
// 卡片会留在 document 里，下一个用例的 `document.activeElement` 断言就在别人的树上跑。
afterEach(cleanup);

/**
 * 被测卡片。
 *
 * `ready` 为假时**整段返回 `null`**——这正是 `AddContentPopover` 在快照到达前的形状，
 * 也是"初始焦点永远进不去"那个真机缺陷（2026-08-26）的最短复现。
 */
function Dialog({
  ready = true,
  children,
  ...options
}: DialogBehaviorOptions & { ready?: boolean; children?: ReactNode }) {
  const attach = useDialogBehavior<HTMLDivElement>(options);
  if (!ready) {
    return null;
  }
  return createElement(
    "div",
    { ref: attach, className: "luna-dialog", tabIndex: -1, "data-testid": "card" },
    children ??
      createElement("button", { type: "button", "data-testid": "first" }, "first"),
  );
}

/**
 * 卡外的触发钮（焦点归还的落点）+ 卡片本体，一起挂进真实 document。
 *
 * 顺序刻意与线上一致：**先让触发钮拿到焦点，卡片再出现**。反过来（渲染完再抢焦点）
 * 会让 hook 把 `<body>` 记成 opener，"焦点归还"这条断言就成了空跑。
 */
function mount(props: Parameters<typeof Dialog>[0]) {
  const tree = (ready: boolean) =>
    createElement(
      "div",
      null,
      createElement("button", { type: "button", "data-testid": "opener" }, "opener"),
      createElement(Dialog, { ...props, ready }),
    );
  const view = render(tree(false));
  const opener = view.getByTestId("opener") as HTMLButtonElement;
  opener.focus();
  const open = () => view.rerender(tree(props.ready ?? true));
  open();
  return { view, opener, tree };
}

describe("初始焦点必须等到卡片真的挂载", () => {
  it("首帧没有卡片、第二帧才有时，焦点仍然落进卡内", () => {
    // 旧写法（`useRef`）在这里恰好全绿地失败：effect 首帧照跑、查了个空、`focus()` 是
    // 空操作；卡片到场后依赖没变、effect 不再跑，于是焦点永远留在触发钮上，用户必须
    // 先按一次 Tab 才能用方向键。
    const onClose = vi.fn();
    const { view, opener, tree } = mount({ onClose, ready: false });
    expect(document.activeElement, "卡片还没来，焦点不该被动过").toBe(opener);

    view.rerender(tree(true));
    expect(document.activeElement).toBe(view.getByTestId("first"));
  });

  it("调用方指定了初始焦点就用它，否则用卡内第一个可聚焦元素", () => {
    const initialFocusRef = { current: null as HTMLElement | null };
    const view = render(
      createElement(Dialog, {
        onClose: () => {},
        initialFocusRef,
        children: [
          createElement("button", { key: "a", type: "button", "data-testid": "first" }, "a"),
          createElement(
            "button",
            {
              key: "b",
              type: "button",
              "data-testid": "second",
              ref: (node: HTMLButtonElement | null) => {
                initialFocusRef.current = node;
              },
            },
            "b",
          ),
        ],
      }),
    );
    expect(document.activeElement).toBe(view.getByTestId("second"));
  });

  it("卡内一个可聚焦元素都没有时，焦点落在卡片本身（读屏据此进入模态）", () => {
    const view = render(
      createElement(Dialog, {
        onClose: () => {},
        children: createElement("p", null, "只有一句话"),
      }),
    );
    expect(document.activeElement).toBe(view.getByTestId("card"));
  });
});

describe("效果在卡片缺席时整段跳过，不做半套", () => {
  it("卡片没挂载时不抢 Escape——同屏另一张卡按 Esc 不该被它吃掉", () => {
    const onClose = vi.fn();
    mount({ onClose, ready: false });
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("`enabled: false` 时同样整段跳过（挂载但未打开的卡）", () => {
    const onClose = vi.fn();
    const { opener } = mount({ onClose, enabled: false });
    expect(document.activeElement, "关着的卡不该抢焦点").toBe(opener);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("Escape 是键盘用户的逃生通道", () => {
  it("按 Escape 调用 onClose", () => {
    const onClose = vi.fn();
    mount({ onClose });
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("`closeOnEscape: false` 时不关（有在途操作的少数卡）", () => {
    const onClose = vi.fn();
    mount({ onClose, closeOnEscape: false });
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("模态圈住 Tab，非模态放行", () => {
  const twoButtons = [
    createElement("button", { key: "a", type: "button", "data-testid": "first" }, "a"),
    createElement("button", { key: "b", type: "button", "data-testid": "last" }, "b"),
  ];

  it("模态：走到末项再按 Tab 回到首项，Shift+Tab 反之", () => {
    const view = render(
      createElement(Dialog, { onClose: () => {}, children: twoButtons }),
    );
    const first = view.getByTestId("first");
    const last = view.getByTestId("last");

    last.focus();
    fireEvent.keyDown(document, { key: "Tab" });
    expect(document.activeElement).toBe(first);

    fireEvent.keyDown(document, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(last);
  });

  it("模态：焦点已经掉到卡外时把它拽回来，而不只是在两端循环", () => {
    const { view } = mount({ onClose: () => {}, children: twoButtons });
    (view.getByTestId("opener") as HTMLButtonElement).focus();
    fireEvent.keyDown(document, { key: "Tab" });
    expect(document.activeElement).toBe(view.getByTestId("first"));
  });

  it("非模态：Tab 不被拦，浏览器照常把焦点带出浮层", () => {
    const view = render(
      createElement(Dialog, { onClose: () => {}, modal: false, children: twoButtons }),
    );
    (view.getByTestId("last") as HTMLButtonElement).focus();
    // 判据是"有没有被 `preventDefault`"：圈定的实现手段就是它，放行的实现就是不碰。
    const prevented = !fireEvent.keyDown(document, { key: "Tab" });
    expect(prevented).toBe(false);
    expect(document.activeElement).toBe(view.getByTestId("last"));
  });
});

describe("非模态点到外面就是「我不要它了」", () => {
  it("点浮层外 → 关闭；点浮层内 → 不关", () => {
    const onClose = vi.fn();
    const { view } = mount({ onClose, modal: false });
    fireEvent.pointerDown(view.getByTestId("first"));
    expect(onClose, "点自己身上不该关").not.toHaveBeenCalled();
    fireEvent.pointerDown(view.getByTestId("opener"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("模态点外面不关（那多半是误触遮罩）", () => {
    const onClose = vi.fn();
    const { view } = mount({ onClose });
    fireEvent.pointerDown(view.getByTestId("opener"));
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("焦点归还", () => {
  it("关掉之后回到打开它的那颗按钮，而不是掉回 body", () => {
    const { view, opener, tree } = mount({ onClose: () => {} });
    expect(document.activeElement).toBe(view.getByTestId("first"));
    // 关掉 = 卡片不再渲染（触发钮仍在原地），正是线上的关闭形状。
    view.rerender(tree(false));
    expect(document.activeElement).toBe(opener);
  });

  it("用户已经主动点向别处时不强行拽回来（那是打断，不是归还）", () => {
    const { view, opener, tree } = mount({
      onClose: () => {},
      children: createElement("button", { type: "button", "data-testid": "first" }, "first"),
    });
    const elsewhere = document.createElement("button");
    document.body.append(elsewhere);
    elsewhere.focus();
    view.rerender(tree(false));
    expect(document.activeElement, "焦点该留在用户自己选的地方").toBe(elsewhere);
    expect(document.activeElement).not.toBe(opener);
    elsewhere.remove();
  });

  it("`onClose` 每帧换身份不得让效果重建——否则每帧都把焦点还回去一次", () => {
    // 这条是把"`onClose` 不进依赖数组"翻成可观察行为：效果若随每次渲染重建，它的清理
    // 函数（= 焦点归还）就会在重渲染时把焦点从卡内拽回触发钮。
    function Host() {
      const [, bump] = useState(0);
      return createElement(
        "div",
        null,
        createElement(
          "button",
          { type: "button", "data-testid": "opener", onClick: () => bump((n) => n + 1) },
          "opener",
        ),
        // 每次渲染都是一个**新的**箭头函数，正是线上调用方的写法。
        createElement(Dialog, { onClose: () => {} }),
      );
    }
    const view = render(createElement(Host));
    const inside = view.getByTestId("first");
    expect(document.activeElement).toBe(inside);
    view.rerender(createElement(Host));
    view.rerender(createElement(Host));
    expect(document.activeElement, "重渲染把焦点弹出去了 = 效果被重建了").toBe(inside);
  });
});
