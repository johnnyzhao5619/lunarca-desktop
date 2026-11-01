import { describe, expect, it } from "vitest";
import zh from "../../i18n/locales/zh-CN/settings.json";
import en from "../../i18n/locales/en-US/settings.json";

/**
 * 外部工具面板的**动态 i18n key** 守护（PRD-09-028）。
 *
 * 为什么需要单独一条：状态徽标的文案由变量拼出（`` t(`tools.state.${state}`) ``），
 * 静态扫描 `t("…")` 的正则**看不到它们**——新增一种状态却漏写文案时，界面上直接显示裸 key，
 * 而 typecheck / lint / 既有测试一个都不会红。与 `fetch-i18n.test.ts` 同一心智。
 */

type Dict = Record<string, unknown>;

/** 按点路径取文案；缺失或非字符串返回 `undefined`。 */
function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

const BUNDLES = [zh, en] as unknown as Dict[];

describe("外部工具面板的动态文案", () => {
  it("各件工具的全部状态徽标在中英两份文案里都在场", () => {
    // yt-dlp 四态 + ffmpeg 的 optional + JS 运行时的 recommended。
    // `recommended` 与 `optional` 必须分立：缺 ffmpeg 只是画质降档（照样下得下来），
    //    缺 JS 运行时却会让 yt-dlp 退到随时可能被站点 403 拒掉的备用客户端。
    for (const state of [
      "ready",
      "missing",
      "optional",
      "recommended",
      "timeout",
      "broken",
      "checking",
    ]) {
      for (const bundle of BUNDLES) {
        expect(lookup(bundle, `tools.state.${state}`), `缺 tools.state.${state}`).toBeTruthy();
      }
    }
  });

  it("面板只留一条别处没写的管辖边界；设置中心不再有「外部工具」页（ADR-167 ⑤）", () => {
    // 组件面板住在「应用 ▸ 从链接导入」的「组件」一节，标题与说明由详情页给。`managed_scope`
    // 回答的是「列表里显示可用的那个 ffmpeg，Lunarca 会不会去动它」，全仓仅此一处。
    for (const bundle of BUNDLES) {
      expect(lookup(bundle, "group_description.external_tools"), "外部工具页已删除").toBeUndefined();
      expect(lookup(bundle, "tools.managed_scope")).toBeTruthy();
      expect(lookup(bundle, "tools.description"), "面板 description 应已并入页描述").toBeUndefined();
    }
  });

  it("中英两份的 tools 段落键集合完全一致（漏译即红）", () => {
    const keysOf = (bundle: Dict): string[] => {
      const walk = (node: unknown, prefix: string): string[] => {
        if (typeof node !== "object" || node === null) {
          return [prefix];
        }
        return Object.entries(node as Dict).flatMap(([k, v]) => walk(v, `${prefix}.${k}`));
      };
      return walk(bundle["tools"], "tools").sort();
    };
    expect(keysOf(zh as unknown as Dict)).toEqual(keysOf(en as unknown as Dict));
  });
});
