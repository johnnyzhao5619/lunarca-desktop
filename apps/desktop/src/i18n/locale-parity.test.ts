import { describe, expect, it } from "vitest";
import { comparableLocaleKeys, pluralBaseKey, type LocaleDict } from "./locale-keys";

/**
 * 全 namespace 的中英键集守护（批次 3/4 验收轮补上的缺口）。
 *
 * 此前只有 workspace.json 有递归 parity 测试（graph-rescan-i18n.test.ts），而
 * settings.json / notifications.json 里同样有大片**动态查表**的键（`option.<key>.<value>`、
 * `notifications:<kind>.{title,body}`、语义索引面板的 `semantic.*`）——漏写一边就是裸 key
 * 或英文界面冒中文，且 typecheck / lint / 既有测试全绿。批次 4 的 semantic.* 两边都写对了
 * 纯属自觉，自觉不是机制。
 *
 * 用 `import.meta.glob` 收全两个语言目录：新增 namespace 自动纳入守护，不必回来登记。
 */

const MODULES = import.meta.glob("./locales/*/*.json", { eager: true }) as Record<
  string,
  { default: LocaleDict }
>;

/** locale → namespace 文件名 → 键集。 */
function byLocale(): Map<string, Map<string, string[]>> {
  const out = new Map<string, Map<string, string[]>>();
  for (const [path, module] of Object.entries(MODULES)) {
    const match = /\.\/locales\/([^/]+)\/([^/]+\.json)$/.exec(path);
    if (!match) {
      continue;
    }
    const [, locale, file] = match;
    if (!out.has(locale!)) {
      out.set(locale!, new Map());
    }
    out.get(locale!)!.set(file!, comparableLocaleKeys(module.default));
  }
  return out;
}

describe("i18n 全 namespace 中英键集一致", () => {
  const locales = byLocale();
  const zh = locales.get("zh-CN");
  const en = locales.get("en-US");

  it("两个语言目录都在场且 namespace 文件清单一致", () => {
    expect(zh, "zh-CN 目录缺失").toBeTruthy();
    expect(en, "en-US 目录缺失").toBeTruthy();
    expect([...zh!.keys()].sort()).toEqual([...en!.keys()].sort());
  });

  it("每个 namespace 的键集递归一致", () => {
    for (const [file, zhKeys] of zh!) {
      expect(en!.get(file), `${file} 中英键集不一致`).toEqual(zhKeys);
    }
  });

  /**
   * 反向夹具：**放过复数后缀，不等于放过缺键**。
   *
   * 比较的是基名（`x_one` ≡ `x`），因为复数形态是逐语言的语法——英语要两条、中文一条
   * 都不要，而要求逐字相同等于禁止正确处理英文单复数。但一条整键缺席仍必须判红，否则
   * 这条豁免就成了一个洞。
   */
  it("放过的只有复数后缀：整条键缺席照样判红", () => {
    expect(pluralBaseKey("main.trashExpiresIn_one")).toBe("main.trashExpiresIn");
    expect(pluralBaseKey("main.deletedAt")).toBe("main.deletedAt");
    // 只有复数形态之差 → 视为一致。
    expect(comparableLocaleKeys({ a: { b: "x", b_one: "y" } })).toEqual(["a.b"]);
    // 少一整条 → 键集不同。
    expect(comparableLocaleKeys({ a: { b: "x" } })).not.toEqual(
      comparableLocaleKeys({ a: { b: "x", c: "y" } }),
    );
  });
});

/**
 * 英文界面不冒中文：键集一致挡不住「英文那一条里写了中文」——2026-09-28 审读时在
 * `checkup.card.broken-link.desc` 发现一对中文直角引号「」，typecheck / lint / 键集守护全绿。
 * 豁免只列**本来就该是中文**的值（例：首启引导里拿来演示翻译的中文原文），逐条具名。
 */
const CJK = /[\u3000-\u303f\u3400-\u9fff\uff00-\uffef]/;
const CJK_ALLOWED_IN_EN = new Set(["onboarding.json:step.first_task.sample_text"]);

function cjkValues(file: string, dict: LocaleDict, prefix = ""): string[] {
  return Object.entries(dict).flatMap(([key, value]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") {
      const id = `${file}:${path}`;
      return CJK.test(value) && !CJK_ALLOWED_IN_EN.has(id) ? [id] : [];
    }
    return cjkValues(file, value as LocaleDict, path);
  });
}

describe("英文界面不含中文字符", () => {
  const english = Object.entries(MODULES).filter(([path]) => path.includes("/en-US/"));

  it("扫描面非空，且每条豁免都还指向一个真实存在的中文值", () => {
    expect(english.length).toBeGreaterThan(10);
    for (const id of CJK_ALLOWED_IN_EN) {
      const [file, key] = id.split(":");
      const module = english.find(([path]) => path.endsWith(`/${file}`));
      const value = key!.split(".").reduce<unknown>(
        (node, part) => (node as LocaleDict | undefined)?.[part],
        module?.[1].default,
      );
      expect(typeof value === "string" && CJK.test(value), `豁免 ${id} 已失效，删掉它`).toBe(true);
    }
  });

  it("en-US 各 namespace 没有中文字符（具名豁免除外）", () => {
    const hits = english.flatMap(([path, module]) =>
      cjkValues(path.split("/").pop()!, module.default),
    );
    expect(hits).toEqual([]);
  });

  it("反向夹具：中文引号与汉字都抓得住，纯英文与弯引号不误伤", () => {
    expect(cjkValues("x.json", { a: "「{{subject}}」 has no note" })).toEqual(["x.json:a"]);
    expect(cjkValues("x.json", { a: { b: "主题页" } })).toEqual(["x.json:a.b"]);
    expect(cjkValues("x.json", { a: "“{{subject}}” has no note — ok" })).toEqual([]);
  });
});
