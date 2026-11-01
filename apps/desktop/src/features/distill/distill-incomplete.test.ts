/// <reference types="vite/client" />
/**
 * 「产物不完整」这一态的前端守护（D9′ ①②④）。
 *
 * 现网缺陷的用户侧形状是：一篇蒸馏笔记在生成中途撞上单次输出上限，正文缺了一截，
 * 而**界面上没有任何地方说得出这件事**。本批把这个事实从 `JobResult` 一路带到 Vault，
 * 这里锁的是最后一段——它真的到了用户眼前，且没有被说成另一件事。
 *
 * 三条判据：
 * ① 徽标读的是 Rust 下发的 `output_truncated`（不是前端自己猜的），且它与 `stale`
 *    **各说各的**；
 * ② 三档归属中英各有各的文案（合并成一句即红——三档对应三个不同的用户动作，
 *    ADR-089 ⑧ / PRD-13 §1.5 E）；
 * ③ **绝不复用 `truncated` 这个词**：客户端既有的 `chat:*.truncated` 指的是
 *    **输入上下文被裁剪**（"较早的消息已被截断"），与"输出没写完"方向相反。两个相反
 *    含义共用一个名字，同屏出现时读的人一定会串。
 */

import { describe, expect, it } from "vitest";
import badgeSource from "./DistillBadge.tsx?raw";
import bindingsSource from "../../ipc/bindings.ts?raw";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import zhChat from "../../i18n/locales/zh-CN/chat.json";
import enChat from "../../i18n/locales/en-US/chat.json";

type Dict = Record<string, unknown>;

/**
 * 触顶归属的三档（`luna_core::TruncatedBy::key` 的全集）。
 *
 * 写在这里是**判定面的一部分**而不是一份副本：少一档就意味着那一档的文案没有人守着，
 * 而 Rust 侧新增一档时这里不跟上，恰恰会被「文案两两不同」那条用例的计数当场抓到。
 */
const ATTRIBUTIONS = ["caller", "upstream", "absolute"] as const;

function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

/** 剥掉注释——注释里逐字写着这些标识符，带注释扫描的话把实现改坏也不会红。 */
const badge = badgeSource
  .split("\n")
  .filter((line) => !line.trim().startsWith("*") && !line.trim().startsWith("//"))
  .map((line) => line.split("//")[0] ?? "")
  .join("\n");

describe("蒸馏产物「不完整」态（D9′）", () => {
  it("判定来自 Rust 下发的 `output_truncated`，前端不自己算", () => {
    // 生成物里有这一格 = Rust 真的在下发它（bindings.ts 由 export-bindings 生成，
    // CI 会重导出并 `git diff --exit-code`，故它与 Rust 结构恒等）。
    const dto = bindingsSource.slice(
      bindingsSource.indexOf("export type DistillStateDto = {"),
      bindingsSource.indexOf("export type DistillTemplateChoiceDto"),
    );
    // 是**归属**不是布尔：`incomplete: boolean` 把三个不同的用户动作压成了一个。
    expect(dto, "DistillStateDto 里没有触顶归属——Rust 侧没下发这个事实").toContain(
      "output_truncated: string | null",
    );
    expect(dto, "归属又被压回一个布尔了").not.toContain("incomplete: boolean");
    expect(badge, "徽标没读 output_truncated").toContain("state.output_truncated");
  });

  it("「不完整」与「源已更新」各说各的，不合并成一句", () => {
    // 合成一句的代价：用户被支去源笔记里找一个根本没发生的改动。
    expect(badge).toContain("t(`distill.outputTruncated.${truncatedBy}`)");
    expect(badge).toContain('t("distill.stale")');
    const stale = lookup(zhWorkspace as unknown as Dict, "distill.stale");
    for (const by of ATTRIBUTIONS) {
      const zh = lookup(zhWorkspace as unknown as Dict, `distill.outputTruncated.${by}`);
      expect(zh, `缺 ${by} 档的措辞`).toBeTruthy();
      expect(zh).not.toBe(stale);
    }
  });

  it("「重来救得了的那两档」才走「该重来」编排；`caller` 档不走", () => {
    // `needsRedo` 是「源变了」与「重来有救的触顶」的并集。只认 `stale` 的话，源没变的
    // 那批不完整产物在徽标上与一篇完整笔记完全一样——那正是现网这个缺陷的形状；
    // 而把 `caller` 也塞进去，则是请用户白等一遍必然一模一样的生成。
    expect(badge).toMatch(
      /redoMayHelp\s*=\s*truncatedBy === "upstream" \|\| truncatedBy === "absolute"/,
    );
    expect(badge).toMatch(/needsRedo\s*=\s*state\.stale\s*\|\|\s*redoMayHelp/);
    expect(badge).toContain("generated && needsRedo && !state.edited");
    // `caller` 档虽不催促重来，仍必须有一条如实标注的支路——少了它就又看不见了。
    expect(badge).toContain("showIncomplete");
  });

  it("三档归属中英各有各的文案，且**两两不同**（合并成一句即红）", () => {
    for (const [name, bundle] of [
      ["zh-CN", zhWorkspace],
      ["en-US", enWorkspace],
    ] as unknown as [string, Dict][]) {
      for (const family of ["distill.outputTruncated", "distill.outputTruncatedHint"]) {
        const copies = ATTRIBUTIONS.map((by) => {
          const text = lookup(bundle, `${family}.${by}`);
          expect(text, `${name} 缺 ${family}.${by}`).toBeTruthy();
          return text;
        });
        expect(
          new Set(copies).size,
          `${name} 的 ${family} 有两档共用了同一句话——三档各对应一个不同的用户动作，` +
            "压成一句就是让所有人去猜该动哪一个",
        ).toBe(ATTRIBUTIONS.length);
      }
    }
  });

  it("措辞绝不与「输入上下文被裁剪」撞车", () => {
    for (const [name, workspace, chat] of [
      ["zh-CN", zhWorkspace, zhChat],
      ["en-US", enWorkspace, enChat],
    ] as unknown as [string, Dict, Dict][]) {
      const output = ATTRIBUTIONS.map((by) =>
        lookup(workspace, `distill.outputTruncated.${by}`),
      ).join(" ");
      const outputHint = ATTRIBUTIONS.map((by) =>
        lookup(workspace, `distill.outputTruncatedHint.${by}`),
      ).join(" ");
      // 既有的输入侧文案（"上下文过长，较早的消息已被截断"）——它必须还在，且与输出侧不同。
      const input = lookup(chat, "flag.truncated");
      expect(input, `${name}: 找不到输入侧的 truncated 文案，命名撞车的判据落空了`).toBeTruthy();
      expect(output, name).not.toBe(input);
      expect(outputHint, name).not.toBe(input);
      // 输出侧不许提"上下文 / 较早的消息 / context"——那是另一件事。
      expect(`${output} ${outputHint}`, `${name}: 输出截断的文案跑去说输入裁剪了`).not.toMatch(
        /上下文|较早的消息|earlier messages|context was too long/i,
      );
    }
  });

  it("i18n key 本身也不许叫 `truncated`（同屏两个相反含义）", () => {
    for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
      const distill = (bundle as Dict).distill as Dict;
      expect(
        Object.keys(distill),
        "distill 命名空间里出现了 `truncated`——它在本仓已经是「输入被裁剪」的名字",
      ).not.toContain("truncated");
    }
  });
});
