/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import { dedicatedBlocks, parseCssRules as parseRules } from "@lunarca/lint-config/css-guards";
import { reachableStyleSources, sourcePathLocation } from "./style-source";

/**
 * 「一个选择器只允许有一个定义点」守护（TDD-15 §3.3b，2026-07-25 真机事故）。
 *
 * 样式由多个文件组成时，同名类若在两处各写一块，**后出现的那块静默覆盖前一块**：
 * 改样式的人看着自己那份不生效，typecheck / lint / build 却全绿。设置行尾的「恢复默认」
 * 就栽在这上面——早期文字按钮块在 300 行外把 28px 图标槽撑成盒子，用户在真机截图里指着
 * 那个方块问「这是什么」。
 *
 * 判据只认**专属块**：同一 at-rule 上下文里，某个选择器独占一整块（`sel { … }`，逗号
 * 列表里只有它自己）出现了两次以上。刻意**不**把分组基线算作违规——
 * `.a, .b { 共享声明 }` 之后再写 `.b { 差异声明 }` 是 CSS 的正常分层写法，把它算进来
 * 只会逼人把共享声明复制 N 份，反倒违背 DRY。
 *
 * 注：判据必须**解析**而不是按行正则匹配。多行分组选择器的最后一行长得和专属块的开头
 * 一模一样（`.luna-md-h6 {`），按行匹配会把 19 处里的 15 处误判成重复定义。解析器与判据在
 * `@lunarca/lint-config/css-guards`（网页的 `lint:css` 用同一份）。
 */

describe("样式表守护 · 一个选择器只有一个定义点", () => {
  const rules = reachableStyleSources.flatMap((source) => parseRules(source.text, source.path));

  it("没有任何选择器拥有两个及以上专属定义块", () => {
    const offenders = [...dedicatedBlocks(rules).entries()]
      .filter(([, occurrences]) => occurrences.length > 1)
      .map(([key, occurrences]) => {
        const [ctx, sel] = key.split("||");
        return `${sel}${ctx ? `（在 ${ctx} 内）` : ""}：${occurrences
          .map((rule) => sourcePathLocation(rule.sourcePath, rule.line))
          .join(" / ")}`;
      });
    expect(
      offenders,
      `发现重复定义（后出现的块会静默覆盖前一块，见 TDD-15 §3.3b）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("守护本身确实解析到了样式表（防空集永远通过）", () => {
    expect(rules.length).toBeGreaterThan(1000);
    expect(dedicatedBlocks(rules).size).toBeGreaterThan(800);
    // at-rule 上下文确有被识别（@media / @container 里的块不与顶层块混为一谈）
    expect(rules.some((rule) => rule.atContext.startsWith("@"))).toBe(true);
  });

  it("守护认得出重复、也认得出分组基线（反向验证）", () => {
    const duplicated = parseRules(`
      .a { color: red; }
      .b { color: blue; }
      .a { padding: 0; }
    `, "fixture.css");
    expect([...dedicatedBlocks(duplicated).values()].filter((l) => l.length > 1)).toHaveLength(1);

    // 分组基线 + 专属细化：正常写法，绝不能报
    const layered = parseRules(`
      .a, .b { color: red; }
      .b { padding: 0; }
      @media (min-width: 40rem) { .b { padding: 1px; } }
    `, "fixture.css");
    expect([...dedicatedBlocks(layered).values()].filter((l) => l.length > 1)).toHaveLength(0);
  });
});
