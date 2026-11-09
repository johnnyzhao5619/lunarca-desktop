/// <reference types="vite/client" />
/**
 * 术语与本地化守护（PRD-00 §9 术语表 / TDD-11 §1.4b）。
 *
 * ① **每个目的地只有一个名字键**。此前多个目的地各有两份：速记（`main.voiceTitle`「语音
 *    速记」/ HUD「AI 语音输入」与空间名并存）、录音（`nav.recording`「录音」/
 *    `main.recordingTitle`「录音文件」）、对话（`nav.chat`「AI对话」/ `chat:title`「AI 对话」
 *    ——同一个名字抄成两份之后连空格都不一样了）；速记、录音、转写升为一级空间后
 *    （ADR-087），旧的上位空间 `nav.materials` 与二级分区键 `nav.section.*` 那四个随之退役，
 *    名字只剩 `nav.quickNotes` / `nav.recording` / `nav.transcripts` 各一份。判据是**退役的
 *    副本键不得复活**：这是可验证的事实，而"两个键是不是一个意思"机器读不出来。
 *
 * ② **退役术语不得回流**。「转录」在界面上曾与「转写」并存（同一个动作两个名字，PRD-00 §9
 *    已定名）；`guidance_zh` 是一个**自证问题的字段名**——它宣称自己装中文，而 UI 把它
 *    原样渲染给英文用户看。
 *
 * ③ **绝对时间的本地化必须带 locale**。七处此前直接 `new Date(ms).toLocaleString()`，
 *    那读的是**系统 locale**：用户把界面切成英文、系统还是中文时，回收站每一行的删除
 *    时间仍是中文格式。界面语言与时间格式是同一件事，不该有两个来源。
 *
 * ⑤ **应用内指路只有一种写法，且指得到真的存在的页**。同一条消息的中英两版曾各用一种
 *    分隔符（`semantic.blocked.no_model_hint`：中文 `›`、英文 `→`），同一个文件里两条权限
 *    提示也各用一种；而两处「设置 › 模型」指向的页在 ADR-120 之后根本不叫这个名字，英文
 *    那句还把「系统」写成了 "System & Diagnostics"。分隔符不统一只是难看，**指到一个不
 *    存在的页名是把用户送进死胡同**——而两者都只有并排看两种语言才看得出来。
 *
 * ④ **字节单位阶梯只有一份**。`format-bytes` 与 `lib/format-datetime.ts` 曾各写一套
 *    `1024` 换算，一份标 `KB/MB/GB`、一份标 `KiB/MiB/GiB`，于是同一个数字在「数据足迹」
 *    与「用量记录」两页读法不同，而其中一份的注释正好在论证另一份是错的。这类重复靠
 *    自觉发现不了——两份各自都是对的，只有并排看才看得出矛盾，而没有人会并排看它们。
 */

import { describe, expect, it } from "vitest";

import { SETTINGS_PAGES } from "../features/settings/settings-information-architecture";

const LOCALES = import.meta.glob(["./locales/*/*.json", "../../../../packages/*/src/locales/*/*.json"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const CODE = import.meta.glob(["../{app,features,components,hooks,lib}/**/*.{ts,tsx}", "../../../../packages/*/src/**/*.{ts,tsx}"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 已退役的重复名字键（每个目的地只留一个名字，见 §① 的清单）。 */
/**
 * 「注销」不得用作删除账户的界面用词。
 *
 * 中文里它最常见的意思是**退出登录**（Windows 的「注销」就是登出），而这个动作是服务端
 * 侧的**硬删**：账户行删掉、邮箱索引释放，同一个邮箱要重新注册。它此前就摆在
 * 「登出本设备」正下方——真机上用户按了它，然后再也登不回来（2026-09-10）。
 * 英文那侧一直写的是 "Delete account"，中文这侧把不可逆说成了一个听起来可逆的词。
 */
export const RETIRED_ACCOUNT_TERMS = ["注销"];

export const RETIRED_NAME_KEYS = [
  "nav.materials",
  "nav.section.quickNotes",
  "nav.section.allNotes",
  "nav.section.recordings",
  "nav.section.imports",
  "nav.chat",
  "main.recordingTitle",
  "tabs.notes",
  "tabs.voice",
  "tabs.chat",
  "tabs.activity",
] as const;

/**
 * 界面上不得再出现的术语（PRD-00 §9 已定名）。
 *
 * - 「转录」：同一个动作曾与「转写」并存。
 * - **成熟度轴的旧名**（ADR-086 / ADR-087）：`待整理状态` 那一行写得很死——界面用
 *   「待整理 / 完成整理 / 转为正式笔记」，**不显示「记录态 / Draft / Inbox」**。
 *   `记录态` 与 `收件箱` 是它的两个旧名；英文侧的 `Draft` 由 [`RETIRED_EN_TERMS`] 管。
 *
 * **这里只放"作为界面名字的旧词"**：`记录` 单独一个词不在名单里，因为它在
 *   「执行记录」「用量记录」「对话记录」「设备记录」里都是合法且不同的意思；机械替换
 *   会把这些一起改掉（PRD-00 §9 术语表的明文要求）。
 *
 * **「动态」不在这份名单里**，它由 [`retiredMotionTerm`] 单独判——理由见那里：
 *   字面串判据抓的是排版，不是词。
 *
 * `试穿` 是主题「先看看效果、不保存」那个动作的旧名，2026-10-21 用户裁决改叫「预览」（英文侧的
 *   `Try on` 同批改为 `Preview`，由 [`RETIRED_EN_TERMS`] 管）。
 */
export const RETIRED_TERMS = ["转录", "记录态", "收件箱", "试穿"] as const;

/** ADR-087 ② 拆掉的那个聚合页的名字。 */
const RETIRED_MOTION_TERM = "动态";

/**
 * 「动态」的**结构化豁免**：合法用法逐条具名，写清哪个键、为什么合法。
 *
 * 豁免必须是**具体短语**，不是一个笼统正则——正则黑洞会把将来所有含「动态」的文案
 * 一起放过去，那就等于把这条判据删了。加一条豁免 = 当场回答「这处为什么不是那个聚合页」。
 */
export const MOTION_TERM_EXEMPTIONS = [
  {
    phrase: "减弱动态效果",
    where: "locales/zh-CN/settings.json → appearance.reduce_motion_desc",
    why:
      "macOS 系统设置里那一项的**原名**。这句文案的作用就是让用户去系统设置里找到它，" +
      "逐字引用才找得到；这里的「动态」是苹果的用词，与本仓退役的那个聚合页无关。",
  },
] as const;

/**
 * 英文界面不得再出现的术语。
 *
 * `Draft` / `Inbox` 是成熟度轴的旧名（同上）。判据带词边界：`Drafts`、`draft` 一并
 * 命中，而 `-draft` 这类**搜索语法 token 属机器语法**、不随译名变化（PRD-06-058 技术
 * 边界），故 token 本体由 [`SEARCH_TOKEN_ALLOWLIST`] 放行，只拦说明文案。
 */
export const RETIRED_EN_TERMS = ["Draft", "Inbox", "Try on"] as const;

/**
 * 搜索语法 token 白名单：它们是**机器语法**，稳定不随译名变化。
 *
 * 帮助与示例文案里描述它们时一律用「待整理」表述，但 token 本体必须原样出现——否则
 * 用户照着帮助敲进去的东西根本搜不到。
 */
export const SEARCH_TOKEN_ALLOWLIST = ["-draft", "is:draft", "draft:"] as const;

/**
 * **必须真解析 JSON，不能用正则**：第一版写的是 `"nav"\s*:\s*\{[^}]*"quickNotes"`，
 * 而 `nav.labelWithUnread` 的值里含 `{{count}}` —— 那个 `}` 会把 `[^}]*` 截断，于是
 * 副本键就在花括号后面藏了过去。反向验证时正是这么发现的：种一个 `nav.quickNotes`
 * 回去，门禁**没有变红**。
 */
export function retiredNameKeys(json: string): string[] {
  const parsed = JSON.parse(json) as Record<string, unknown>;
  return RETIRED_NAME_KEYS.filter((key) => {
    // 键可以嵌套任意层（`nav.section.imports`）：沿路径走到父节点，再看叶子在不在。
    const path = key.split(".");
    const leaf = path.pop() ?? "";
    const bucket = path.reduce<unknown>(
      (node, part) =>
        typeof node === "object" && node !== null
          ? (node as Record<string, unknown>)[part]
          : undefined,
      parsed,
    );
    return typeof bucket === "object" && bucket !== null && leaf in bucket;
  });
}

/**
 * 递归取出一份 locale 里的**全部值**（不含键名）。
 *
 * **只看值**：内部键名本来就用技术词（`draft` / `showDrafts` / `draftGroup`），
 * PRD-00 §9 也明说「内部 `draft` / `luna.stage` 只表达技术状态」。拿整份 JSON 原文去
 * 匹配，键名会把每一个正常的键都判成违规——那样的守护只能被关掉，不会被修好。
 */
function localeValues(json: string): string[] {
  const out: string[] = [];
  const walk = (node: unknown) => {
    if (typeof node === "string") {
      out.push(node);
    } else if (node && typeof node === "object") {
      Object.values(node as Record<string, unknown>).forEach(walk);
    }
  };
  walk(JSON.parse(json));
  return out;
}

export function retiredTerms(json: string): string[] {
  const values = localeValues(json).join("\n");
  return RETIRED_TERMS.filter((term) => values.includes(term));
}

/** 扣掉结构化豁免之后的文案——豁免短语整段抠掉，剩下的「动态」才算数。 */
function withoutMotionExemptions(text: string): string {
  return MOTION_TERM_EXEMPTIONS.reduce(
    (rest, { phrase }) => rest.split(phrase).join(" "),
    text,
  );
}

/**
 * 界面文案里回流的「动态」（ADR-087 ② 拆掉的那个聚合页），返回违规的**整句文案**。
 *
 * **为什么不能是字面串**：这条判据原本写的是带直角引号的 `"「动态」"`，于是
 * `动态中心`、`动态页`、`动态入口`、`"动态"`、`【动态】` 全都抓不到——它抓的是**排版**，
 * 不是词。而它当时之所以没有误伤「减弱动态效果」（macOS 系统设置名），靠的也正是
 * 引号形态碰巧不匹配：**巧合，不是设计**。改一次排版，两头同时失效。
 *
 * **也不能靠枚举形态族**（`「动态」|动态中心|动态页|动态入口|动态视图|…`）：
 * 下一个写文案的人会发明一个你没列进去的写法（`动态流`、`动态 tab`、`动态面板`），
 * 而漏掉的那一种不会以任何形式现形。所以判据**反过来**：界面文案里出现「动态」一律
 * 视为回流，合法用法必须进 [`MOTION_TERM_EXEMPTIONS`] 具名报备。带不带引号、后面接
 * 什么词，都在覆盖面里；代价只是每新增一处合法用法要写一行理由——那正是想要的。
 *
 * **扫描面只有 locale 的「值」**（见 [`localeValues`]）：内部键名（`nav.section.*`
 * 这类标识符）、源码里的英文符号、用户自己写的笔记正文都不在里面。稳定搜索 token
 * （`-draft` 等，见 [`SEARCH_TOKEN_ALLOWLIST`]）也不含中文，与本判据无交集。
 */
export function retiredMotionTerm(json: string): string[] {
  return localeValues(json).filter((value) =>
    withoutMotionExemptions(value).includes(RETIRED_MOTION_TERM),
  );
}

/**
 * 英文文案里的退役术语（带词边界，并放行搜索语法 token 本体）。
 *
 * 先把 token 本体从文本里抠掉再匹配：`-draft` 里的 `draft` 不是那个被退役的**名字**，
 * 它是用户要原样敲进搜索框的语法。
 */
export function retiredEnglishTerms(json: string): string[] {
  let text = localeValues(json).join("\n");
  for (const token of SEARCH_TOKEN_ALLOWLIST) {
    text = text.split(token).join(" ");
  }
  return RETIRED_EN_TERMS.filter((term) => new RegExp(`\\b${term}s?\\b`, "i").test(text));
}

/** 源码里的注释（判据只看真正会执行的代码——注释里举反例是解释，不是违规）。 */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/\/\/.*/g, " ");
}

/** 不带 locale 参数的绝对时间/数字本地化调用。 */
export function localeFreeFormatting(source: string): string[] {
  return [
    ...stripComments(source).matchAll(/\.toLocale(?:String|DateString|TimeString)\(\s*\)/g),
  ].map((m) => m[0]);
}

/** `@lunarca/editor-core/format-bytes` 之外自造的字节单位阶梯（相邻两个引号包起来的单位就是它的形状）。 */
export function adHocByteUnitLadder(source: string): string[] {
  return [
    ...stripComments(source).matchAll(
      /(["'])(?:B|[KMGTP]i?B)\1\s*,\s*(["'])[KMGTP]i?B\2/g,
    ),
  ].map((m) => m[0]);
}

/** 字节单位阶梯的唯一归属（判据里写死路径，改名时本用例会当场变红）。 */
const BYTE_UNITS_HOME = "../../../../packages/editor-core/src/format/format-bytes.ts";


// ---- ⑤ 应用内指路（分隔符 + 目的地必须真的存在）----

/** 同一 locale 下 settings 资源包的路径——页名只住在那一份里。 */
function settingsBundleFor(localePath: string): string {
  return localePath.replace(/\/[^/]+\.json$/, "/settings.json");
}

/** 设置页在界面上的名字（中英各一份，事实源是 `group.<页>` 文案）。 */
function pageLabels(localeJson: string): string[] {
  const group = (JSON.parse(localeJson) as { group?: Record<string, string> }).group ?? {};
  return SETTINGS_PAGES.map((page) => group[page]).filter((label): label is string =>
    Boolean(label),
  );
}

/** 排除 `系统设置` / `System Settings` —— 它们指的是 macOS，不是我们这一页。 */
const OWN_SETTINGS_ROOT_GUARD = "(?<!系统)(?<!System )";

/**
 * ` → ` 的**具名豁免**：对照与顺序，不是指路。
 *
 * 判据刻意是「全局禁 → + 逐条豁免」，而不是「起点必须是某个页名」：后者抓不住以面板名
 * 起头的指路（`连接服务端 → 服务端身份`），更抓不住**页名本身已经过时**的那种——英文曾写
 * `AI engine / models → Server connection`，名字都不对了，按名字匹配的判据自然匹配不上。
 * 豁免少而具体，比一张永远补不齐的起点表可靠。
 */
const ARROW_EXEMPTIONS: { path: string; why: string }[] = [
  { path: "chat.sources.summary", why: "检索 N 篇 → 引用 M 篇：一次收敛的前后两个数" },
  { path: "cloud.billing.rules.order_separator", why: "套餐额度 → 预付余额：账本扣减顺序（服务端 ledger_order）" },
  { path: "popup.meta.summary", why: "{{mode}} → {{target}}：翻译方向" },
  { path: "popup.meta.summaryUncertain", why: "同上，带不确定标记的一版" },
  { path: "settings.popup_translate.direction_preview", why: "其他语言 → 母语：翻译方向，不是应用内导航" },
  { path: "settings.tools.js_runtime_why_manual", why: "Deno → Node → Bun：挑选顺序" },
];

/** 资源包里带 ` → ` 的文案路径（`<包名>.<键路径>`）。 */
export function arrowBearingPaths(bundle: string, json: string): string[] {
  const found: string[] = [];
  const walk = (node: unknown, path: string) => {
    if (typeof node === "string") {
      if (node.includes(" → ")) found.push(path);
      return;
    }
    if (node && typeof node === "object") {
      for (const [key, value] of Object.entries(node)) {
        walk(value, path ? `${path}.${key}` : key);
      }
    }
  };
  walk(JSON.parse(json), bundle);
  return found;
}

/** 未获豁免的 ` → `——一律视作写错了分隔符的应用内指路。 */
export function unexemptedArrows(bundle: string, json: string): string[] {
  const exempt = new Set(ARROW_EXEMPTIONS.map((item) => item.path));
  return arrowBearingPaths(bundle, json).filter((path) => !exempt.has(path));
}

/** 资源包名（`chat` / `settings` / …），用于把豁免路径对上文件。 */
function bundleName(localePath: string): string {
  return localePath.replace(/^.*\//, "").replace(/\.json$/, "");
}

/**
 * 指到一个不存在的页：`设置 › X` / `Settings › X` 里的 X 不是任何一页的名字。
 *
 * 只管这一种形状——它是「从别处指回设置中心」的唯一写法，也是 ADR-120 改名之后
 * 最容易陈旧的那一处。
 */
export function pointerToUnknownPage(localeJson: string, labels: string[]): string[] {
  const bad: string[] = [];
  for (const root of ["设置", "Settings"]) {
    // macOS 的「系统设置 › 隐私与安全性 › …」不是指向我们的页，后行断言把它排除掉。
    const re = new RegExp(`${OWN_SETTINGS_ROOT_GUARD}${root} › ([^"」。，,.;；]+)`, "g");
    for (const match of localeJson.matchAll(re)) {
      const target = match[1].trim();
      // 要么指到这一页为止，要么接着一个 `›` 往页内分节下探。**不能只判前缀**：
      // 「System & Diagnostics」以「System」开头，而它不是任何一页的名字。
      const known = labels.some((label) => target === label || target.startsWith(`${label} ›`));
      if (!known) {
        bad.push(`${root} › ${target}`);
      }
    }
  }
  return bad;
}

describe("术语与本地化", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(Object.keys(LOCALES).length).toBeGreaterThan(20);
    expect(Object.keys(CODE).length).toBeGreaterThan(50);
    expect(Object.keys(LOCALES).some((path) => path.includes("/packages/editor-core/src/")), "共享渲染包不在扫描面里").toBe(true);
    expect(Object.keys(CODE).some((path) => path.includes("/packages/editor-core/src/")), "共享渲染包不在扫描面里").toBe(true);
  });

  it("每个目的地只有一个名字键（退役的副本键没有复活）", () => {
    const offenders = Object.entries(LOCALES).flatMap(([path, json]) =>
      retiredNameKeys(json).map((key) => `${path}: ${key}`),
    );
    expect(
      offenders,
      `这些键是另一个键的第二份名字；请引用该目的地自己的那个键：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("界面文案不再出现已退役的术语（PRD-00 §9）", () => {
    const offenders = Object.entries(LOCALES).flatMap(([path, json]) =>
      retiredTerms(json).map((term) => `${path}: ${term}`),
    );
    expect(
      offenders,
      `同一个动作只有一个名字（转写）；退役术语见 PRD-00 §9：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("「动态」没有作为界面名字回流（ADR-087 ②），合法用法都已具名豁免", () => {
    const offenders = Object.entries(LOCALES).flatMap(([path, json]) =>
      retiredMotionTerm(json).map((value) => `${path}: ${value}`),
    );
    expect(
      offenders,
      `「动态」是 ADR-087 ② 拆掉的那个聚合页，指着它的说明文字会让用户去找一个不存在的\n` +
        `地方。确属合法用法（如系统设置原名）就进 MOTION_TERM_EXEMPTIONS 具名报备并写理由：\n` +
        offenders.join("\n"),
    ).toEqual([]);
  });

  /**
   * 豁免不许留成空转：文案改了、那处合法用法没了，豁免条目必须跟着删——否则它会在
   * 下一次「动态」回流时替真违规挡枪，而没有任何地方会提醒你它已经没有对象了。
   */
  it("每条「动态」豁免都还有对象（文案改了就删豁免）", () => {
    const all = Object.values(LOCALES).flatMap(localeValues).join("\n");
    for (const { phrase, where } of MOTION_TERM_EXEMPTIONS) {
      expect(all.includes(phrase), `豁免「${phrase}」（${where}）已经没有对象，删掉它`).toBe(
        true,
      );
    }
  });

  it("英文界面不再出现成熟度轴的旧名（Draft / Inbox）", () => {
    const offenders = Object.entries(LOCALES)
      .filter(([path]) => path.includes("en-US"))
      .flatMap(([path, json]) => retiredEnglishTerms(json).map((term) => `${path}: ${term}`));
    expect(
      offenders,
      `PRD-00 §9：界面用「待整理 / 转为正式笔记」的任务语言，不显示 Draft / Inbox：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  /**
   * **反向夹具**：证明上面两条不是空跑。
   *
   * 退役术语的守护有一个天然的失败模式——**它在名单为空、正则写错或扫描面丢失时同样
   * 全绿**。种一份含旧词的假文案进去，它必须变红；同时种一份只含搜索 token 的文案，
   * 它必须**不**变红（否则帮助文案里的 `-draft` 会被误伤）。
   */
  it("反向夹具：旧词重新出现时判据会红，而搜索语法 token 不误伤", () => {
    expect(retiredTerms('{"a":"记录态：还没定稿"}')).toEqual(["记录态"]);
    expect(retiredTerms('{"a":"这是收件箱"}')).toEqual(["收件箱"]);
    expect(retiredEnglishTerms('{"a":"Drafts are piling up"}')).toEqual(["Draft"]);
    expect(retiredEnglishTerms('{"a":"Open the Inbox"}')).toEqual(["Inbox"]);

    // 合法用法一个都不许误伤。
    expect(retiredTerms('{"a":"执行记录与用量记录"}')).toEqual([]);
    expect(retiredEnglishTerms('{"a":"Type -draft to skip unsorted items"}')).toEqual([]);
    expect(retiredEnglishTerms('{"a":"Use is:draft in search"}')).toEqual([]);
  });

  /**
   * **反向夹具：「动态」的形态族与结构化豁免**。
   *
   * 上一版判据是字面串 `"「动态」"`，下面第一组里**只有第一条会红**，其余四条全都从
   * 它眼皮底下过去了——那不是判据宽松，那是判据在抓排版。第二组证明豁免走的是具名
   * 清单而不是排版巧合：把「减弱动态效果」拆开写（`动态效果减弱`）就必须红，
   * 因为它不再是被报备的那个系统设置原名。
   */
  it("反向夹具：「动态」不论怎么排版都抓得住，具名豁免与扫描面边界都成立", () => {
    // ① 塞一条「动态」导航名进 locale，一律要红——带不带引号、后面接什么词都算。
    for (const copy of [
      "都在「动态」里看",
      "打开动态中心",
      "动态页已经拆分",
      "从动态入口进去",
      "切到「动态」视图",
      "在 动态 里能看到全部更新",
    ]) {
      expect(retiredMotionTerm(JSON.stringify({ a: copy })), `漏掉了：${copy}`).toEqual([copy]);
    }

    // ② 具名豁免的那一处仍然绿；换个写法就不再是那处系统设置名，必须红。
    expect(retiredMotionTerm('{"a":"系统已开启「减弱动态效果」时自动生效。"}')).toEqual([]);
    expect(retiredMotionTerm('{"a":"把动态效果减弱一些"}')).toEqual(["把动态效果减弱一些"]);

    // ③ 扫描面只有「值」：内部标识符是键，不进判据（PRD-00 §9：内部技术词不受术语表约束）。
    expect(retiredMotionTerm('{"nav": {"section": {"topics": "主题页"}}}')).toEqual([]);
    expect(retiredMotionTerm('{"动态": "更新"}')).toEqual([]);
    expect(retiredMotionTerm('{"a": {"dynamicFeed": "最新"}}')).toEqual([]);

    // ④ 稳定搜索 token 与英文符号里没有中文，与本判据无交集。
    expect(retiredMotionTerm('{"a":"输入 -draft 或 is:draft 过滤"}')).toEqual([]);
  });

  /**
   * 「服务端在替你跑这件事」只有**一个**名字（ADR-059 ④）。
   *
   * 真机反馈（2026-08-11）：同一屏上四种说法——「服务端托管（推荐）」（下拉空选项）、
   * 「服务端托管（自动选择）」（当前生效）、「服务端稳定向量档」（嵌入槽）、
   * 「服务端转写」（转写槽，且是 Rust 写死的中文）。读起来像四种不同的东西，而它们是
   * 同一个状态。三个落点的差别（谁选模型 / 向量档锁定 / 音频通道）改由
   * `models.managed.note.*` 说明——**差别写在说明里，不写进名字里**。
   *
   * 判据是「退役键不得复活 + 剩下那几处必须逐字相同」：两条都是机器读得出来的事实。
   */
  it("「服务端托管」在各处逐字一致，且分叉的旧名字没有复活（ADR-059）", () => {
    const at = (ns: string, locale: string, path: string): unknown =>
      path
        .split(".")
        .reduce<unknown>(
          (node, key) =>
            typeof node === "object" && node !== null
              ? (node as Record<string, unknown>)[key]
              : undefined,
          JSON.parse(LOCALES[`./locales/${locale}/${ns}.json`]) as unknown,
        );

    // 退役的三个「第二个名字」不得复活。
    for (const [path, json] of Object.entries(LOCALES)) {
      if (!path.includes("settings.json")) continue;
      const parsed = JSON.parse(json) as Record<string, unknown>;
      const managed = (parsed.models as Record<string, unknown> | undefined)?.managed as
        | Record<string, unknown>
        | undefined;
      for (const retired of ["effective", "profile_effective", "follow_policy"]) {
        expect(
          managed?.[retired],
          `${path}: models.managed.${retired} 是「服务端托管」的第二个名字，已退役——` +
            `差别请写进 models.managed.note.*，不要再造一个名字`,
        ).toBeUndefined();
      }
    }

    // 说法分叉过的三处，逐字比对——写这条时它当场抓出 en-US 的第五种说法。
    for (const locale of ["zh-CN", "en-US"]) {
      const canonical = at("settings", locale, "models.managed.name");
      expect(canonical, `${locale}: models.managed.name 缺失`).toBeTruthy();
      // **算力卡不在这张表里**（ADR-126 第 14 条）：那一格与卡头同屏，而卡头说的是
      // 「AI 请求发往 Lunarca 云」这样一个**具体的地方**。两处不是同一个词时，用户读不出
      // 它们说的是同一件事（真机可见）。这不是给「服务端托管」又造一个名字——「Lunarca 云」
      // 与「组织服务端」本来就是两台不同的机器，而下面那条断言正是钉住这一点。
      // 其余三处都不与卡头同屏，它们说的是「谁选模型」这件事，泛称仍然正确。
      const sameWord: [string, string, string][] = [
        ["settings", "models.managed.title", "运行位置标题"],
        ["chat", "route.managed_service", "AI 对话顶栏"],
        ["workspace", "statusbar.compute.managedService", "工作区状态栏"],
      ];
      for (const [ns, path, where] of sameWord) {
        expect(at(ns, locale, path), `${locale}: ${where}用了另一个说法`).toBe(canonical);
      }
      // 算力卡那两个落点名必须**互不相同**，否则"按落点取词"就退化成两个 key 写同一个词；
      // 泛称「服务端托管」也不得从那里复活（它说的是"谁选模型"，不是"哪一台"）。
      const routeNames = ["server_cloud", "server_organization"].map((key) =>
        at("settings", locale, `compute_card.route.${key}`),
      );
      for (const [index, name] of routeNames.entries()) {
        expect(name, `${locale}: 缺 compute_card.route.${index}`).toBeTruthy();
        expect(name, `${locale}: 算力卡的落点名与「谁选模型」那个泛称重合了`).not.toBe(canonical);
      }
      expect(new Set(routeNames).size, `${locale}: 两个落点名必须互不相同`).toBe(2);
      expect(
        at("settings", locale, "compute_card.route.server"),
        `${locale}: 算力卡的泛称 compute_card.route.server 已随 ADR-126 第 14 条退役`,
      ).toBeUndefined();

      // 三种形态各自的差别必须有说明——否则收敛名字就变成了丢信息。
      for (const mode of ["managed", "profile", "channel"]) {
        expect(
          at("settings", locale, `models.managed.note.${mode}`),
          `${locale}: 缺 models.managed.note.${mode}，收敛名字不能把差别一起抹掉`,
        ).toBeTruthy();
      }
    }
  });

  it("没有 `guidance_zh` 这种自证问题的字段名", () => {
    const offenders = Object.entries(CODE)
      .filter(([, source]) => /\bguidance_zh\b/.test(stripComments(source)))
      .map(([path]) => path);
    expect(
      offenders,
      `可本地化的走 i18n key，语言无关的具体值走 message_detail：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("绝对时间与数字的本地化都带上了界面语言", () => {
    const offenders = Object.entries(CODE).flatMap(([path, source]) =>
      localeFreeFormatting(source).map((call) => `${path}: ${call}`),
    );
    expect(
      offenders,
      `不传 locale 读的是系统语言，与界面语言无关（TDD-11 §1.4b）；用 lib/format-datetime：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  /**
   * **字节单位阶梯只此一处**（2026-09-09 收口，见文件头 §④）。
   *
   * 判据刻意不管「谁在按 1024 除」——那个形状在别处也合法（时长、KiB 计数的进度）；
   * 它管的是**谁在给单位起名字**。名字一旦有第二份，两份就会各自漂，而界面上看不出
   * 哪一份才算数。
   */
  it("字节单位阶梯只有 @lunarca/editor-core/format-bytes 一份", () => {
    const offenders = Object.entries(CODE)
      .filter(([path]) => path !== BYTE_UNITS_HOME)
      .flatMap(([path, source]) => adHocByteUnitLadder(source).map((hit) => `${path}: ${hit}`));
    expect(
      offenders,
      `体积一律走 @lunarca/editor-core/format-bytes 的 formatBytes（IEC 单位 + 注入 locale）；` +
        `按 1024 算却标 GB 是三家风格指南都禁止的写法：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  /** 判据不许空转：唯一那份必须真的在扫描面里，且真的含着那道阶梯。 */
  it("字节单位阶梯的唯一归属还在原处", () => {
    const home = CODE[BYTE_UNITS_HOME];
    expect(home, `${BYTE_UNITS_HOME} 不在扫描面里——判据已失去对象`).toBeTruthy();
    expect(adHocByteUnitLadder(home ?? "")).not.toEqual([]);
  });

  it("反向验证：四条判据各自抓得住违规、放得过合规", () => {
    expect(retiredNameKeys('{"nav": {"vault": "笔记库", "materials": "资料"}}')).toEqual([
      "nav.materials",
    ]);
    // 嵌套一层的退役键同样抓得住；同名叶子在**别的**层级不算（`nav.imports` 不是 `nav.section.imports`）。
    expect(retiredNameKeys('{"nav": {"section": {"imports": "导入与转写"}}}')).toEqual([
      "nav.section.imports",
    ]);
    expect(retiredNameKeys('{"nav": {"imports": "x", "section": {"topics": "主题页"}}}')).toEqual([]);
    expect(retiredNameKeys('{"nav": {"vault": "笔记库"}}')).toEqual([]);
    // 同名叶子在**别的**段里不算（`main.chat` 不是 `nav.chat`）。
    expect(retiredNameKeys('{"main": {"chat": "x"}}')).toEqual([]);

    expect(retiredTerms('{"a": "批量转录"}')).toEqual(["转录"]);
    expect(retiredTerms('{"a": "批量转写"}')).toEqual([]);

    expect(localeFreeFormatting("new Date(ms).toLocaleString()")).toHaveLength(1);
    expect(localeFreeFormatting("new Date(ms).toLocaleString(locale)")).toEqual([]);
    expect(localeFreeFormatting("formatDateTime(ms, i18n.language)")).toEqual([]);
    // 注释里举的反例是解释，不是违规。
    expect(localeFreeFormatting("// 别写 new Date(ms).toLocaleString()")).toEqual([]);

    // 两种口径的阶梯都要抓得住——收口保留的是 IEC 那一份，但**再写一份 IEC 的**同样是重复。
    // 只断言「抓到了」不断言条数：正则不重叠匹配，四个单位的阶梯自然报两处。
    expect(adHocByteUnitLadder('const U = ["B", "KB", "MB", "GB"];')).not.toEqual([]);
    expect(adHocByteUnitLadder('const U = ["B", "KiB", "MiB"] as const;')).not.toEqual([]);
    // 单独提到某个单位不算（文案、测试断言、注释里的举例都会提到它们）。
    expect(adHocByteUnitLadder('expect(formatBytes(2048, "en-US")).toBe("2 KiB");')).toEqual([]);
    expect(adHocByteUnitLadder('// 按 1024 算却标 "GB", "TB" 是错的')).toEqual([]);
  });

  // ---- ⑤ 应用内指路 ----

  it("应用内指路一律用 ›，不用 →", () => {
    for (const [path, json] of Object.entries(LOCALES)) {
      expect(
        unexemptedArrows(bundleName(path), json),
        `${path} 里有用 → 写的指路——同一个动作在别处写的是 ›；确属对照/顺序就进 ARROW_EXEMPTIONS 并写明理由`,
      ).toEqual([]);
    }
  });

  /** 豁免不许空转：文案改了却留着豁免，等于给下一处违规先开好了口子。 */
  it("每条 → 豁免都还有对象（文案改了就删豁免）", () => {
    const live = new Set(
      Object.entries(LOCALES).flatMap(([path, json]) => arrowBearingPaths(bundleName(path), json)),
    );
    for (const { path, why } of ARROW_EXEMPTIONS) {
      expect(live.has(path), `豁免 ${path}（${why}）已经没有对象了`).toBe(true);
    }
  });

  it("指路指到的页真的存在", () => {
    for (const [path, json] of Object.entries(LOCALES)) {
      const labels = pageLabels(LOCALES[settingsBundleFor(path)] ?? "{}");
      expect(
        pointerToUnknownPage(json, labels),
        `${path} 指向了一个不存在的设置页（ADR-120 之后页名变过）`,
      ).toEqual([]);
    }
  });

  it("反向验证：指路判据抓得住违规、放得过对照用法", () => {
    const labels = ["AI 算力与模型", "系统", "AI compute & models", "System"];

    // 起点是页名、面板名，或一个**已经过时的**页名——三种都得抓住。
    expect(unexemptedArrows("common", '{"a": "请到「设置 → 模型」安装"}')).toEqual(["common.a"]);
    expect(unexemptedArrows("cloud", '{"e": {"x": "「连接服务端 → 服务端身份」"}}')).toEqual([
      "cloud.e.x",
    ]);
    expect(
      unexemptedArrows("cloud", '{"e": {"x": "under AI engine / models → Server connection"}}'),
    ).toEqual(["cloud.e.x"]);
    expect(unexemptedArrows("common", '{"a": "请到「设置 › AI 算力与模型」安装"}')).toEqual([]);
    // 对照与顺序：路径在豁免表里才放过，换个键照样红。
    expect(unexemptedArrows("popup", '{"meta": {"summary": "{{mode}} → {{target}}"}}')).toEqual([]);
    expect(unexemptedArrows("popup", '{"meta": {"other": "{{mode}} → {{target}}"}}')).toEqual([
      "popup.meta.other",
    ]);

    expect(pointerToUnknownPage('{"a": "「设置 › 模型」"}', labels)).toEqual(["设置 › 模型"]);
    expect(pointerToUnknownPage('{"a": "from Settings › System & Diagnostics"}', labels)).toEqual([
      "Settings › System & Diagnostics",
    ]);
    expect(pointerToUnknownPage('{"a": "「设置 › AI 算力与模型」"}', labels)).toEqual([]);
    expect(pointerToUnknownPage('{"a": "from Settings › System"}', labels)).toEqual([]);
    // 起点不是「设置」的指路不归这条判据（页内分节名由别处保证）。
    expect(pointerToUnknownPage('{"a": "「连接服务端 › 服务端身份」"}', labels)).toEqual([]);
  });

  /** 「注销」不是删除账户的用词——它在中文里读作「退出登录」。 */
  it("删除账户不叫「注销」", () => {
    for (const [path, json] of Object.entries(LOCALES)) {
      if (!path.includes("zh-CN")) continue;
      const bundle = JSON.parse(json) as { delete?: unknown };
      // 逐层走到每一句：`delete` 下也有按成因分的子表（`delete.blocked.<reason>`），只看第一层
      // 会让子表里的句子绕过这条守护。
      const visit = (value: unknown, key: string): void => {
        if (typeof value === "string") {
          for (const term of RETIRED_ACCOUNT_TERMS) {
            expect(value.includes(term), `${path} ${key} 用了「${term}」`).toBe(false);
          }
          return;
        }
        if (value !== null && typeof value === "object") {
          for (const [child, nested] of Object.entries(value)) visit(nested, `${key}.${child}`);
        }
      };
      visit(bundle.delete ?? {}, "delete");
    }
  });

  /**
   * 「用量」这个词只属于账户页（ADR-122 ⑥）。
   *
   * AI 页那一面回答「跑了什么」，账户页回答「套餐还剩多少」；两处都叫「用量」时，
   * 用户搜一个词会得到两个地方、两个数字。判据只管**标题**——正文里说「Token 用量」
   * 是在描述一个量，不是在给一个去处起名字。
   */
  it("AI 页那一面的标题里不出现「用量」", () => {
    const zh = JSON.parse(LOCALES["./locales/zh-CN/settings.json"]) as {
      models: { title: string; view: Record<string, string> };
    };
    expect(zh.models.view.usage).not.toContain("用量");
    // 模型库也按 ADR-122 叫「模型库」，不是动作名「模型管理」。
    expect(zh.models.title).toBe("模型库");
  });
});
