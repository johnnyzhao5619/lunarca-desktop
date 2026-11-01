// @vitest-environment happy-dom
/**
 * 有序多选控件的交互契约（TDD-05 §7.5a「控件交互」/ PRD-07-068 [P0] / PRD-09-070）。
 *
 * 这一族全是**真实渲染**：它要证明的四件事在源码断言下没有一件看得见——
 * ① 注册表下发的 `min` / `max` 有没有真的被消费（丢了它，开关就没有上下限可依）；
 * ② 增删有没有走"提交完整新数组"那条通路（而不是另开一条增量协议）；
 * ③ 移到首/末位时焦点有没有掉回 `<body>`（那颗按钮当场被置灰）；
 * ④ 播报是不是等 Rust 真的答应了才说（不 await 就乐观播报 = 拒了也说"已移除"）。
 *
 * 形态与「有序开关列表」同一套：全集逐行、每行一枚开关 + 上移/下移；已选在前按序，
 * 未选灰在后。开 = 追加到末尾，关 = 移除。
 *
 * **写入是异步经 Rust 回来的**：`set_setting` 成功后由 `settings://changed` 触发重读，
 * 提交那一刻列表还是旧的。夹具照这个节奏演——先 resolve 命令，再更新 `current_value`。
 * 演成同步的话，第 ③ 条会假绿：焦点在旧布局上落一次就"成功"了。
 */

import { createElement, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

import { t, withI18n } from "../../i18n/i18n-test-fixture";
import type { SettingSpecDto } from "../../ipc/bindings";
import { SettingControl, type SetValue } from "./SettingControls";

const ACTION_IDS = [
  "text_quick_note",
  "voice_quick_note",
  "new_note",
  "start_recording",
  "import_material",
];

/** 注册表默认值（`QUICK_START_DEFAULT`）：写作三选二 + 导入一。 */
const DEFAULT_FOUR = ["text_quick_note", "voice_quick_note", "new_note", "import_material"];

/** 与 Rust 注册表逐字相同的那一项（`min: 2, max: 4`，见 `luna-config` 的 registry）。 */
function quickStartSpec(value: string[]): SettingSpecDto {
  return {
    key: "workspace.quick_start_actions",
    group: "workspace",
    scope: "device",
    modified: false, available: true, default_value: { kind: "string_list", value: DEFAULT_FOUR },
    resettable: true,
    current_value: { kind: "string_list", value },
    requires_restart: false,
    secret: false,
    surface: { kind: "row" },
    visibility: "standard",
    section: null,
    i18n_key: "settings:workspace.quick_start_actions",
    desc_i18n_key: "settings:workspace.quick_start_actions_desc",
    control: {
      kind: "ordered_multi_select",
      options: ACTION_IDS,
      min: 2,
      max: 4,
      require_each_category: true,
    },
  } as unknown as SettingSpecDto;
}

/** 该动作在界面上叫什么（取自真资源包，用例因此不硬写译文）。 */
function label(id: string): string {
  return t(`settings:option.workspace.quick_start_actions.${id}`);
}

/**
 * 宿主：把控件挂起来，并模拟 Rust 那一趟往返。
 *
 * `verdict` 决定 Rust 答不答应；答应了才把新值写回 `current_value`（并且是在命令
 * resolve **之后**的一次独立渲染里，与线上 `settings://changed` 的节奏一致）。
 */
function mount(initial: string[], verdict: (next: string[]) => boolean = () => true) {
  const submissions: string[][] = [];
  function Host() {
    const [value, setValue] = useState(initial);
    const apply: SetValue = async (_key, next) => {
      const list = next.kind === "string_list" ? next.value : [];
      submissions.push(list);
      if (!verdict(list)) {
        return null;
      }
      setValue(list);
      return next;
    };
    return createElement(SettingControl, {
      spec: quickStartSpec(value),
      setValue: apply,
      onVaultChanged: () => {},
    });
  }
  const view = render(withI18n(createElement(Host)));
  const rows = () => [...view.container.querySelectorAll("li.luna-ordered-select__row")];
  const button = (option: string, name: string) =>
    view.getByLabelText(
      t(`settings:orderedMultiSelect.${name}`, { label: label(option) }),
    ) as HTMLButtonElement;
  const status = () => view.container.querySelector('[role="status"]')?.textContent ?? "";
  /** 已选段的顺序（未选段灰在后面，不算"顺序"）。 */
  const order = () =>
    rows()
      .filter((row) => !row.hasAttribute("data-unselected"))
      .map((row) => row.querySelector(".luna-ordered-select__label")?.textContent ?? "");
  return { view, submissions, rows, button, status, order };
}

afterEach(cleanup);

describe("全集逐行：已选在前按序，未选灰在后", () => {
  it("五个动作都在场，四个已选带序号、一个未选没有", () => {
    const ui = mount(DEFAULT_FOUR);
    expect(ui.rows()).toHaveLength(ACTION_IDS.length);
    expect(ui.order()).toEqual(DEFAULT_FOUR.map(label));
    const last = ui.rows()[ui.rows().length - 1];
    expect(last.hasAttribute("data-unselected")).toBe(true);
    expect(last.querySelector(".luna-ordered-select__label")?.textContent).toBe(
      label("start_recording"),
    );
    expect(ui.button("start_recording", "toggle").getAttribute("aria-checked")).toBe("false");
    expect(ui.button("new_note", "toggle").getAttribute("aria-checked")).toBe("true");
  });

  it("未选行的上移/下移禁用——它还没有位置可言", () => {
    const ui = mount(DEFAULT_FOUR);
    expect(ui.button("start_recording", "moveUp").disabled).toBe(true);
    expect(ui.button("start_recording", "moveDown").disabled).toBe(true);
  });
});

describe("控件消费注册表下发的 min / max（N01 / N11）", () => {
  it("四项已满时未选行的开关禁用——上限来自 `max`，不是前端写死的 4", () => {
    const ui = mount(DEFAULT_FOUR);
    expect(ui.button("start_recording", "toggle").disabled).toBe(true);
    // 已选行照样能关（还没到下限）。
    expect(ui.button("new_note", "toggle").disabled).toBe(false);
  });

  it("只剩两项时已选行的开关禁用——下限来自 `min`", () => {
    const ui = mount(["new_note", "import_material"]);
    expect(ui.button("new_note", "toggle").disabled).toBe(true);
    expect(ui.button("import_material", "toggle").disabled).toBe(true);
    // 未选行照样能开（还没到上限）。
    expect(ui.button("voice_quick_note", "toggle").disabled).toBe(false);
  });
});

describe("增删与重排共用同一条「提交完整新数组」的通路（N01）", () => {
  it("开 = 提交旧数组 + 新项（追加在末尾）", async () => {
    const ui = mount(["new_note", "import_material"]);
    fireEvent.click(ui.button("voice_quick_note", "toggle"));
    await waitFor(() =>
      expect(ui.submissions).toEqual([["new_note", "import_material", "voice_quick_note"]]),
    );
    await waitFor(() =>
      expect(ui.order()).toEqual([
        label("new_note"),
        label("import_material"),
        label("voice_quick_note"),
      ]),
    );
  });

  it("关 = 提交去掉它之后的完整数组", async () => {
    const ui = mount(["new_note", "voice_quick_note", "import_material"]);
    fireEvent.click(ui.button("voice_quick_note", "toggle"));
    await waitFor(() => expect(ui.submissions).toEqual([["new_note", "import_material"]]));
    // 关掉的那一行没有消失，只是灰到未选段——它仍在全集里。
    await waitFor(() =>
      expect(ui.button("voice_quick_note", "toggle").getAttribute("aria-checked")).toBe("false"),
    );
  });

  it("重排同样是整数组，一次一条", async () => {
    const ui = mount(["new_note", "import_material"]);
    fireEvent.click(ui.button("import_material", "moveUp"));
    await waitFor(() =>
      expect(ui.submissions[ui.submissions.length - 1]).toEqual(["import_material", "new_note"]),
    );
  });

  it("类目下限由 Rust 裁决，前端不自己算——被拒时值原封不动", async () => {
    // 前端若自己算类目，这条规则就在两处各有一份，而前端那份必然先陈旧。
    const ui = mount(["new_note", "text_quick_note", "import_material"], (next) =>
      next.includes("import_material"),
    );
    fireEvent.click(ui.button("import_material", "toggle"));
    await waitFor(() =>
      expect(ui.submissions, "照样把完整候选数组交出去，由 Rust 说不").toEqual([
        ["new_note", "text_quick_note"],
      ]),
    );
    expect(ui.order()).toEqual([
      label("new_note"),
      label("text_quick_note"),
      label("import_material"),
    ]);
  });
});

describe("焦点保留（N10）", () => {
  it("移到末位时焦点转到仍可用的对侧按钮，不掉回 body", async () => {
    // 「下移」把这一项送到末位的同一刻，那颗按钮就被 `disabled` 了——浏览器随即把焦点
    // 丢回 `<body>`，键盘用户下一次 Tab 得从页首重走一遍。
    const ui = mount(["new_note", "voice_quick_note", "import_material"]);
    const down = ui.button("voice_quick_note", "moveDown");
    down.focus();
    fireEvent.click(down);

    await waitFor(() => expect(ui.order()[ui.order().length - 1]).toBe(label("voice_quick_note")));
    await waitFor(() =>
      expect(document.activeElement).toBe(ui.button("voice_quick_note", "moveUp")),
    );
    expect(document.activeElement).not.toBe(document.body);
  });

  it("移到中间位置时焦点仍留在**原来那颗按钮**上（可以连着按）", async () => {
    const ui = mount(DEFAULT_FOUR);
    const up = ui.button("import_material", "moveUp");
    up.focus();
    fireEvent.click(up);
    await waitFor(() => expect(ui.order()[2]).toBe(label("import_material")));
    expect(document.activeElement).toBe(ui.button("import_material", "moveUp"));
  });

  it("关掉一项后焦点留在它自己的开关上——那一行还在，只是灰了", async () => {
    const ui = mount(["new_note", "voice_quick_note", "import_material"]);
    const toggle = ui.button("voice_quick_note", "toggle");
    toggle.focus();
    fireEvent.click(toggle);
    await waitFor(() => expect(ui.order()).toHaveLength(2));
    const landed = document.activeElement as HTMLElement | null;
    expect(landed, "焦点掉回 body 了").not.toBe(document.body);
    expect(landed).toBe(ui.button("voice_quick_note", "toggle"));
  });
});

describe("播报（N10）", () => {
  it("重排播报新位置", async () => {
    const ui = mount(["new_note", "import_material"]);
    fireEvent.click(ui.button("import_material", "moveUp"));
    await waitFor(() =>
      expect(ui.status()).toBe(
        t("settings:orderedMultiSelect.moved", {
          label: label("import_material"),
          position: 1,
          total: 2,
        }),
      ),
    );
  });

  it("增删各自播报", async () => {
    const ui = mount(["new_note", "import_material"]);
    fireEvent.click(ui.button("voice_quick_note", "toggle"));
    await waitFor(() =>
      expect(ui.status()).toBe(
        t("settings:orderedMultiSelect.added", {
          label: label("voice_quick_note"),
          position: 3,
          total: 3,
        }),
      ),
    );
    fireEvent.click(ui.button("voice_quick_note", "toggle"));
    await waitFor(() =>
      expect(ui.status()).toBe(
        t("settings:orderedMultiSelect.removed", {
          label: label("voice_quick_note"),
          total: 2,
        }),
      ),
    );
  });

  it("Rust 拒了就**不**播报成功——`commit` 必须等结果回来再说话", async () => {
    const ui = mount(["new_note", "text_quick_note", "import_material"], (next) =>
      next.includes("import_material"),
    );
    fireEvent.click(ui.button("import_material", "toggle"));
    await waitFor(() => expect(ui.submissions).toHaveLength(1));
    // 理由由行级 `role="alert"`（`SettingRow` 的 `.luna-field-error`）念出来，这里不重复；
    // 但绝不能留下一句"已移除…还剩 2 项"——屏幕上一项都没少。
    expect(ui.status()).toBe("");
  });
});

describe("i18n（N01）", () => {
  it("控件文案在中英两份 settings.json 都在场", async () => {
    const zh = (await import("../../i18n/locales/zh-CN/settings.json")).default as Record<
      string,
      unknown
    >;
    const en = (await import("../../i18n/locales/en-US/settings.json")).default as Record<
      string,
      unknown
    >;
    for (const key of [
      "orderedMultiSelect.toggle",
      "orderedMultiSelect.moveUp",
      "orderedMultiSelect.moveDown",
      "orderedMultiSelect.moved",
      "orderedMultiSelect.added",
      "orderedMultiSelect.removed",
    ]) {
      expect(typeof zh[key], `zh-CN 缺 ${key}`).toBe("string");
      expect(typeof en[key], `en-US 缺 ${key}`).toBe("string");
    }
  });

  it("两条拒绝码翻得出人话——否则用户看到的是 Rust 的英文原文", () => {
    const errors = (key: string) => t(`settings:error.${key}`);
    expect(errors("duplicate_option")).not.toBe("error.duplicate_option");
    expect(errors("required_category")).not.toBe("error.required_category");
  });
});
