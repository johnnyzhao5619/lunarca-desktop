/// <reference types="vite/client" />
/**
 * 键盘可达守护（TDD-15 §3.3a / PRD-00 §7）。
 *
 * 判据只有一条，但它挡住的是同一类错误：**声明了 ARIA 角色，就得兑现那个角色承诺的
 * 交互**。声明而不兑现比不声明更糟——读屏会照着角色告诉用户"这里能用方向键"，然后
 * 按下去什么也不发生。
 *
 * 现场有两种：
 * ① 六个 `role="tablist"` 里只有一个接了方向键，另外五个只有角色声明；其中笔记标签条
 *    更把 `role="tab"` 放在 `<div>` 上——Tab 键连进都进不去。
 * ② 三处自绘的 `role="menu"`（批量导出、回放导出、说话人指派）都只是 `<details>` 或
 *    裸浮层：没有 Esc、没有方向键、条目不在 roving tabindex 里。前两处收编到全应用唯一的
 *    菜单实现；第三处**根本不是菜单**（单选名册 + 会留着的范围段控 + 两个动作），改回
 *    它本来的身份——对话框浮层。
 */

import { describe, expect, it } from "vitest";

const TSX = import.meta.glob(["../{app,features,components}/**/*.tsx", "../../../../packages/*/src/**/*.tsx"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 界面源码（测试文件里出现的角色名是断言字符串，不是界面）。 */
const VIEWS = Object.fromEntries(
  Object.entries(TSX).filter(([path]) => !path.endsWith(".test.tsx")),
);

/** 菜单行为的实现（它自己当然写着 `role="menu"`）。 */
const MENU_PRIMITIVE = "./ui/ContextMenu.tsx";

/** 源码里的注释（判据只看会执行的代码——注释里举反例是解释，不是违规）。 */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/\/\/.*/g, " ");
}

/**
 * 声明了 `role="tab"` 却没接共享键盘导航的文件。
 *
 * `(?!list)` 不能省：`role="tablist"` 以 `role="tab` 开头，少了它连容器都会被算成
 * 一个标签——反向验证里 `<header role="tablist">` 正是这么被误报出来的。
 */
export function tablistsWithoutSharedNavigation(source: string): boolean {
  // 同样先剥注释：`[<(]` 会把注释里提一句名字、下一行正好以 `<` 开头的情况认成调用。
  const code = stripComments(source);
  return /role="tab"(?!list)/.test(code) && !/useTablistNavigation\s*[<(]/.test(code);
}

/**
 * `role="tab"` 落在非 `<button>` 元素上（`<div role="tab">` 根本不可聚焦）。
 *
 * **不能用"从 `<` 一路吃到 `role`"的正则**：属性值里的箭头函数带 `>`、事件处理器里
 * 还嵌着 JSX，第一版因此从外层 `<div>` 一路吃到了内层按钮的 `role="tab"`，把已经修好的
 * 标签条又报成违规。改为从 `role="tab"` **往回**找最近的 `<`，中途遇到 `>` 就说明那已经
 * 是别的元素了。
 */
export function tabsOnNonButtons(source: string): string[] {
  const found: string[] = [];
  // 下标与切片必须来自**同一份**字符串：第一版在剥注释后的串上取 index、却回原串
  // 切片——注释被换成一个空格，长度一变，所有下标就整体错位，于是三处已经修好的
  // `<button role="tab">` 被报成 `<div>` / `<header>` / `<section>`。
  const code = stripComments(source);
  for (const match of code.matchAll(/role="tab"(?!list)/g)) {
    const before = code.slice(0, match.index);
    const open = before.lastIndexOf("<");
    if (open < 0 || before.slice(open).includes(">")) {
      continue;
    }
    const tag = /^<([A-Za-z][\w.]*)/.exec(before.slice(open))?.[1];
    if (tag && tag !== "button") {
      found.push(`<${tag} role="tab">`);
    }
  }
  return found;
}

/**
 * 声明了菜单角色、却既不委托共享实现、也不自己接方向键的文件。
 *
 * 判据不是"只准有一份菜单"——HUD 那份是**另一种落位与密度**的菜单，它自己把方向键接
 * 全了，那是实现差异不是欺骗。判据是"**声明了就得兑现**"：批量导出、回放导出、标题级别
 * 三处此前只有 `<details>` 的开合，方向键与 Esc 一概没有。
 */
export function menusWithoutKeyboard(source: string): boolean {
  const code = stripComments(source);
  if (!/role="menu(item|itemradio|itemcheckbox)?"/.test(code)) {
    return false;
  }
  const delegates = /useContextMenu\s*\(|<ContextMenuPanel\b/.test(code);
  const handlesKeys = /"ArrowDown"|"ArrowUp"/.test(code);
  return !delegates && !handlesKeys;
}

describe("键盘可达", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(Object.keys(TSX).length).toBeGreaterThan(50);
    expect(TSX[MENU_PRIMITIVE]).toBeTypeOf("string");
    expect(Object.keys(TSX).some((path) => path.includes("/packages/editor-core/src/")), "共享渲染包不在扫描面里").toBe(true);
  });

  it("每个标签组都接了共享的方向键导航", () => {
    const offenders = Object.entries(TSX)
      .filter(([, source]) => tablistsWithoutSharedNavigation(source))
      .map(([path]) => path);
    expect(
      offenders,
      `声明 role="tab" 就要兑现 ←/→/Home/End；用 useTablistNavigation：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("标签本身是真按钮（不是套了角色的 div）", () => {
    const offenders = Object.entries(VIEWS).flatMap(([path, source]) =>
      tabsOnNonButtons(source).map((el) => `${path}: ${el}`),
    );
    expect(
      offenders,
      `不可聚焦的元素套 role="tab" 等于对读屏撒谎：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("声明了菜单角色的地方都真的能用键盘走", () => {
    const offenders = Object.entries(VIEWS)
      .filter(([path]) => path !== MENU_PRIMITIVE)
      .filter(([, source]) => menusWithoutKeyboard(source))
      .map(([path]) => path);
    expect(
      offenders,
      `菜单走 useContextMenu；混合控件的浮层不是菜单，用 role="dialog"：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("反向验证：三条判据各自抓得住违规、放得过合规", () => {
    expect(tablistsWithoutSharedNavigation('<button role="tab" />')).toBe(true);
    expect(tablistsWithoutSharedNavigation('useTablistNavigation(X, a, b);<button role="tab" />')).toBe(
      false,
    );
    // 只提到名字不算接上（必须是调用）。
    expect(tablistsWithoutSharedNavigation('// useTablistNavigation\n<button role="tab" />')).toBe(
      true,
    );

    expect(tabsOnNonButtons('<div role="tab" />')).toEqual(['<div role="tab">']);
    expect(tabsOnNonButtons('<button type="button" role="tab" />')).toEqual([]);
    // 容器不是标签：`role="tablist"` 不能被前缀匹配吃进来。
    expect(tabsOnNonButtons('<div role="tablist">')).toEqual([]);
    expect(tablistsWithoutSharedNavigation('<div role="tablist">')).toBe(false);

    expect(menusWithoutKeyboard('<div role="menu">')).toBe(true);
    expect(menusWithoutKeyboard('<div role="dialog">')).toBe(false);
    expect(menusWithoutKeyboard('useContextMenu();<div role="menu">')).toBe(false);
    expect(menusWithoutKeyboard('if (key === "ArrowDown") {}<div role="menu">')).toBe(false);
    // 注释里提到旧写法不算违规。
    expect(menusWithoutKeyboard('// 此前是 role="menu"')).toBe(false);
  });
});
