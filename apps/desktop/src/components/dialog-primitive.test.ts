/// <reference types="vite/client" />
/**
 * 对话框原语的守护（TDD-15 §3.3a）。
 *
 * **它挡住的是"原语以某个 feature 命名"**：这张卡片此前**事实上已经是**共享原语，
 * 只是叫 `.luna-graph-rescan`——蒸馏批量 / 蒸馏预览 / 知识归入 / 知识体检 / 管理库 /
 * 切库六处都挂着"图谱重扫"的类名当卡片用。名字说谎的代价不是不好看：下一个写对话框的人
 * 读到"图谱重扫"就不会去用它，于是另抄一份六条声明——本仓库里已经这么抄了三份。
 *
 * **第一版只锁了整块类，还明写着「`__子元素` 类照旧属于图谱重扫自己」——于是零件层
 * 原样重演了同一场事故**：页头 / 范围胶囊 / 计数 / 说明 / 错误 / 动作区六个零件被五个
 * feature 共用，名字照旧写着"图谱重扫"。判据与整块类本是同一条，故本轮扩到零件层。
 *
 * 同批加的第三条判据管**行为**：`aria-modal` 的对话框必须经共享的 `useDialogBehavior`。
 * 十二张卡此前八张各写一份 Esc 监听、四张**根本没有 Esc**，而焦点圈定全仓只有一处。
 * 这类缺失不会让任何断言变红——它只会让键盘用户困在一张关不掉的卡里。
 */

import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob("../{app,features,components}/**/*.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/**
 * 借用「图谱重扫」这个 feature 名当共享卡片或共享零件用。
 *
 * 仍然放过的是**真正属于图谱重扫自己**的那两个零件（被停用文件夹清单），故判据按名单
 * 收窄到已收编的六个通用零件 + 整块类。
 */
const SHARED_PARTS = ["header", "scope", "counts", "note", "error", "actions"] as const;

export function borrowsFeatureNamedCard(source: string): boolean {
  if (/\bluna-graph-rescan\b(?!__)/.test(source)) {
    return true;
  }
  return SHARED_PARTS.some((part) =>
    new RegExp(`\\bluna-graph-rescan__${part}\\b`).test(source),
  );
}

/**
 * 声明了 `aria-modal` 却没接共享对话框行为 hook 的组件。
 *
 * 判据认的是**调用**（`useDialogBehavior(` / `useDialogBehavior<`）而不是文件里出现过
 * 这个词——只按词判会被 import 行喂饱：删掉调用、留着 import，门禁照样绿。
 */
export function modalWithoutSharedBehavior(source: string): boolean {
  return (
    source.includes('aria-modal="true"') && !/useDialogBehavior\s*[<(]/.test(source)
  );
}

/** 原生 `window.confirm`（夺焦、无法主题化、没有焦点圈定）。 */
export function usesNativeConfirm(source: string): boolean {
  return /\bwindow\.confirm\s*\(/.test(source);
}

describe("对话框原语 · 名字、零件与行为", () => {
  it("扫描面非空（glob 失效时本用例先红，而不是让下面的断言凭空通过）", () => {
    expect(Object.keys(SOURCES).length).toBeGreaterThan(50);
  });

  it("没有任何组件把 `.luna-graph-rescan` 当共享卡片或共享零件用", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([, source]) => borrowsFeatureNamedCard(source))
      .map(([path]) => path);
    expect(
      offenders,
      `共享卡片用 .luna-dialog、共享零件用 .luna-dialog__*（components/dialog.css）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("每个 aria-modal 对话框都接了共享的 useDialogBehavior", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([, source]) => modalWithoutSharedBehavior(source))
      .map(([path]) => path);
    expect(
      offenders,
      `模态必须有 Esc / 焦点圈定 / 焦点归还，三者由 hooks/use-dialog-behavior 统一给：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("没有任何组件再用原生 window.confirm", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([, source]) => usesNativeConfirm(source))
      .map(([path]) => path);
    expect(
      offenders,
      `破坏性确认走 useConfirm()（components/ConfirmHost.tsx）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("反向验证：三条判据各自抓得住违规、放得过合规", () => {
    expect(borrowsFeatureNamedCard('className="luna-graph-rescan luna-checkup"')).toBe(true);
    expect(borrowsFeatureNamedCard('className="luna-graph-rescan__actions"')).toBe(true);
    expect(borrowsFeatureNamedCard('className="luna-dialog luna-dialog__actions"')).toBe(false);
    // 真正属于图谱重扫自己的零件照旧放行。
    expect(borrowsFeatureNamedCard('className="luna-graph-rescan__exclusions"')).toBe(false);

    expect(modalWithoutSharedBehavior('<div aria-modal="true" />')).toBe(true);
    expect(
      modalWithoutSharedBehavior('useDialogBehavior({}); <div aria-modal="true" />'),
    ).toBe(false);
    expect(modalWithoutSharedBehavior("<div />")).toBe(false);

    expect(usesNativeConfirm("if (window.confirm(msg)) {}")).toBe(true);
    expect(usesNativeConfirm("await ask({ title })")).toBe(false);
  });
});
