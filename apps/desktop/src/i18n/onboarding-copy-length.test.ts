/// <reference types="vite/client" />
/**
 * 首启引导的文案长度上限（产品/运营口径，2026-09-11）。
 *
 * 引导是**扫读**的：用户此刻要的是"下一步做什么"，不是这件事的完整说明。而文案没有任何
 * 阻力会让它变短——每次补一个边界条件就往正文里塞一句，最后一屏正文 90 字、三张卡片各
 * 60 字，没人会因此变红。这一批把七页正文从平均 62 字压到 33 字（总计 −35%），门禁存在
 * 的意义只有一个：**别再爬回去**。
 *
 * 判据刻意分语言给不同的数：中文一个字就是一个信息单位，英文要靠字母拼，直接比长度是
 * 拿两把不同的尺子量同一件事。上限留了余量（当前最长的那条是 41 / 105），抓的是"又长回
 * 60~90 字"这一类，不是逼着每句都写到极限。
 *
 * 边界（诚实交代）：它只管**正文与卡片描述**这两类扫读文本。按钮、状态词、错误提示不在
 * 内——那些本来就短，而错误提示宁可长也要说清怎么办。
 */

import { describe, expect, it } from "vitest";

const LOCALES = import.meta.glob("./locales/*/onboarding.json", {
  eager: true,
  import: "default",
}) as Record<string, Record<string, unknown>>;

/** 扫读文本：每一步的正文、欢迎页三条承诺、「第一个任务」三张卡的描述。 */
const SCANNABLE = [
  /^step\.[a-z_]+\.body$/,
  /^step\.welcome\.promise\.[a-z_]+\.desc$/,
  /^step\.first_task\.(voice|translate|clip)_desc$/,
];

/** 语言 → 上限。中文按字、英文按字符，两把尺子不混用。 */
const LIMIT: Record<string, number> = { "zh-CN": 45, "en-US": 115 };

function flatten(node: unknown, prefix = ""): [string, string][] {
  if (typeof node === "string") return [[prefix, node]];
  if (node === null || typeof node !== "object") return [];
  return Object.entries(node as Record<string, unknown>).flatMap(([k, v]) =>
    flatten(v, prefix ? `${prefix}.${k}` : k),
  );
}

describe("首启引导文案长度", () => {
  it("正文与卡片描述保持在扫读得完的长度内", () => {
    const offenders: string[] = [];
    for (const [path, bundle] of Object.entries(LOCALES)) {
      const parts = path.split("/");
      const locale = parts[parts.length - 2] ?? "";
      const limit = LIMIT[locale];
      if (limit === undefined) {
        offenders.push(`${locale}: 没给上限——新增语言要在 LIMIT 里定一把自己的尺子`);
        continue;
      }
      for (const [key, text] of flatten(bundle)) {
        if (SCANNABLE.some((re) => re.test(key)) && text.length > limit) {
          offenders.push(`${locale} ${key}: ${text.length} > ${limit}\n    ${text}`);
        }
      }
    }
    expect(
      offenders,
      `引导是扫读的，这几条已经长到要停下来读了：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  /** 反向：门禁真的在扫到东西（空集合会让上面那条永远绿）。 */
  it("扫描面非空", () => {
    const keys = Object.values(LOCALES).flatMap((b) =>
      flatten(b).map(([k]) => k).filter((k) => SCANNABLE.some((re) => re.test(k))),
    );
    expect(keys.length).toBeGreaterThanOrEqual(2 * 13);
  });
});
