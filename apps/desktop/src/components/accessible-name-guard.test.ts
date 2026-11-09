/// <reference types="vite/client" />
/**
 * 可访问名守护（TDD-15 §3.3a）。
 *
 * **只有一枚图标的按钮此前一半没有名字**：标签页栏四枚、对话页十三枚、历史条目两枚
 * 只写了 `title`。`title` 确实能兜底出可访问名，但它是 HTML-AAM 里最弱的一档来源，而且
 * 同一个文件里另一枚按钮写的是 `aria-label` + `title` 两份——"该写哪个"没有规则可循时，
 * 下一个人只会照抄离得最近的那份，于是永远修不完。
 *
 * 根治不是逐个补 `aria-label`，是让这件事**没有第二种写法**：`IconButton` 的 `label`
 * 必填，一处写、同时喂给可访问名与工具提示。本守护锁的就是那扇门只有一扇。
 */

import { describe, expect, it } from "vitest";

const TSX = import.meta.glob(["../{app,features,components}/**/*.tsx", "../../../../packages/*/src/**/*.tsx"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/**
 * 原语自身（它就是那扇门，里面必然出现裸 `<button>`）。
 *
 * 键是 glob 归一化后的相对路径：本文件在 `components/` 下，故 `../components/ui/…`
 * 会被归一成 `./ui/…`——写成前者时查不到，"扫描面非空"那条自检当场先红。
 */
const PRIMITIVE = "./ui/IconButton.tsx";

/** 一段源码里所有 `<button …>` 开标签及其闭合前的内容。 */
function buttonElements(source: string): { head: string; body: string }[] {
  const found: { head: string; body: string }[] = [];
  const open = /<button\b(?:[^<>{]|\{(?:[^{}]|\{(?:[^{}]|\{[^{}]*\})*\})*\})*?>/g;
  for (const match of source.matchAll(open)) {
    const end = source.indexOf("</button>", match.index + match[0].length);
    if (end < 0) {
      continue;
    }
    found.push({ head: match[0], body: source.slice(match.index + match[0].length, end) });
  }
  return found;
}

/**
 * 只渲染图标、却没有可访问名的按钮（即"读屏只念得出「按钮」"的那种）。
 *
 * **判"有没有可见文字"不能靠"去掉标签和花括号后还剩不剩字"**：第一版就是这么写的，
 * 于是 `{preset.label}`、`{tab.title}` 这类**表达式文本**被当成空，二十多个有文字的按钮
 * 全被误报。改为逐个看子元素：把每个 JSX 元素换成占位符，只有"剩下的全是图标元素、
 * 或图标之间的三元"才算图标钮——`§{tab.label}` 里那个花括号不含占位符，一眼就是文字。
 */
export function namelessIconButtons(source: string): string[] {
  return buttonElements(source)
    .filter(({ head, body }) => {
      if (/aria-label|aria-labelledby/.test(head)) {
        return false;
      }
      const names = [...body.matchAll(/<([A-Za-z][\w.]*)/g)].map((m) => m[1]);
      if (names.length === 0 || !names.every((n) => n.endsWith("Icon"))) {
        return false;
      }
      // 元素换占位符后，剩下的每个花括号组都必须是"图标之间的三元"，否则它在产出文字。
      const rest = body.replace(/<[^<>]*>/g, "\u00a7");
      const braces = [...rest.matchAll(/\{[^{}]*\}/g)].map((m) => m[0]);
      const braceOk = braces.every((b) => /\u00a7/.test(b));
      return braceOk && !/[\w一-鿿]/.test(rest.replace(/\{[^{}]*\}/g, ""));
    })
    .map(({ head }) => head.replace(/\s+/g, " ").slice(0, 80));
}

/** 仍在自绘图标按钮外壳（`.luna-icon-btn` 直接写在 `className` 上）的地方。 */
export function handRolledIconButtons(source: string): string[] {
  return buttonElements(source)
    .filter(({ head }) => /className="[^"]*\bluna-icon-btn\b/.test(head))
    .map(({ head }) => head.replace(/\s+/g, " ").slice(0, 80));
}

describe("可访问名", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(Object.keys(TSX).length).toBeGreaterThan(50);
    expect(TSX[PRIMITIVE]).toBeTypeOf("string");
    expect(Object.keys(TSX).some((path) => path.includes("/packages/editor-core/src/")), "共享渲染包不在扫描面里").toBe(true);
  });

  it("只有图标的按钮都念得出自己是什么", () => {
    const offenders = Object.entries(TSX)
      .filter(([path]) => path !== PRIMITIVE)
      .flatMap(([path, source]) => namelessIconButtons(source).map((el) => `${path}: ${el}`));
    expect(
      offenders,
      `图标按钮请用 <IconButton label={…} icon={…} />——名字必填，同时是提示：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("图标按钮只有 IconButton 这一扇门（没有人再自绘外壳）", () => {
    const offenders = Object.entries(TSX)
      .filter(([path]) => path !== PRIMITIVE)
      .flatMap(([path, source]) => handRolledIconButtons(source).map((el) => `${path}: ${el}`));
    expect(
      offenders,
      `.luna-icon-btn 由 IconButton 单点挂载；自己写 className 会绕过必填的 label：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("反向验证：两条判据各自抓得住违规、放得过合规", () => {
    expect(namelessIconButtons('<button type="button"><CloseIcon /></button>')).toHaveLength(1);
    expect(namelessIconButtons('<button aria-label={x}><CloseIcon /></button>')).toEqual([]);
    // 有可见文字的按钮不归本判据管（名字由文字承担）。
    expect(namelessIconButtons("<button><CloseIcon />关闭</button>")).toEqual([]);
    expect(namelessIconButtons('<button><CloseIcon />{t("action.close")}</button>')).toEqual([]);

    expect(handRolledIconButtons('<button className="luna-icon-btn">x</button>')).toHaveLength(1);
    expect(handRolledIconButtons('<button className="luna-tabbar__tab">x</button>')).toEqual([]);
  });
});
