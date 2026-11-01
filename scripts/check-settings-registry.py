#!/usr/bin/env python3
"""设置注册表一致性校验（注册表 ↔ 双语文案 ↔ 前端信息架构）。

事实源是代码侧 settings registry（crates/luna-config）。本脚本由 CI 调用，漂移即红。
曾经还拿注册表去对产品规格里的设置总表（原第 1、5 项）；规格不随本仓分发后，
那两项随之移除——剩下的都是代码与代码之间的一致性，不依赖任何文档。

用法：
    python3 scripts/check-settings-registry.py <settings-registry.json> [<locales 目录>]
    python3 scripts/check-settings-registry.py --self-test

校验项：
  2) 每个设置有非空 i18n_key；
  3) 每个 onboarding.* key 为 device 作用域；
  6) （给定 locales 目录时）每个非内部设置的 i18n_key 在
     zh-CN 与 en-US 的 settings.json 中都存在——杜绝设置页渲染裸 key。
  7) （同上）每个用户可见设置都有**一句说明**（`<i18n_key>_desc`），且枚举型设置的
     **每个选项**都有本地化标签（`option.<key>.<value>`，纯语言码可走 `common:lang.*`）——
     否则设置页会出现「一句占位说明」或下拉里蹦出裸取值（PRD-09-070）；声明了组内分节
     （`section`，PRD-09-070 ⑦）的设置，其节标题（`section.<group>.<id>`）同样双语在场——
     否则分节标题渲染为裸 key。
  8) 某分组的**全部**键都不渲染为设置行（内部键 / 面板承载 / 全局投影控件）时，它必须
     登记在前端 `SETTINGS_PANEL_PAGES` 里——否则 `orderedPages` 在**任何投影**下都判定
     该组"没有内容"，**整组连同它的专属面板一起从设置页消失**，`settings://<组>` 深链
     同时落空（模型组正是这样蒸发过一次）。只在标准投影下隐藏的**高级分层分组**
     （实验室 / 系统与诊断 / 外部工具，TDD-05 §7.6）是刻意的渐进披露，不在此列；
     标准投影必须在场的面板型分组（如模型组）由前端 IA 测试逐组钉住。
  9) 反向：登记为面板型的页必须真的登记了面板——`SettingsView.tsx` 的 `PAGE_PANELS`，
     或 `WorkspaceApp.tsx` 的 `SETTINGS_PAGE_PANELS`（整页由外部 feature 承载的账户页）；
     并且 `PAGE_PANELS` 里 `afterSection` 的每个键都必须是该页在注册表里真有的 `section`
     ——键写错时那个面板**一个渲染分支都不进**，而 typecheck（`Record<string, …>`）与
     全部用例都不会红
     （条目形如 `<组>: { … <XxxPanel …> }`），否则等于声明了一个永远空白的分组。
 10) 同义词（`<i18n_key>_aliases` 与 `group_aliases.<页>`）中英**成对**在场：只译一半时
     搜索会在一种语言下悄悄失灵，而两边的 key 集合此前没有任何一处比对过。
 11) **分段控件的选项标签在中英两份真实文案下都排得下**：控件列定宽（16rem），四个标签
     里只要有一个是短语就会换行成两排，而这只有真机截图看得见——中文排得下、英文裂开
     是常态（"Extra large"）。判据认**标签总长**，不认选项个数。
 12) **标准投影的说明字符预算**（中英各自量）：简易层是给"打开设置几秒内找到那一项"
     的人看的，一行说明拖成四五行时那一页就没法扫读了。高级项**不设上限**——它们解释的
     正是简易层刻意藏起来的概念（VAD 判据、文件名模板、签名公钥），压短等于把话说不清。
 13) `ordered_multi_select` 必须导出**非空** `options`，且 `min`/`max` 在场且成立
     （`1 <= min <= max`）。第 7b 条的选项 i18n 校验是**遍历 `options`** 的：导出为空数组
     时它对该项一条都不检查，静静地全绿——`export_settings_registry.rs` 的 `options`
     此前只 match 了 `Select`，五个创建动作 id 的标签因此从未被校验过。这条把"空跑"
     本身变成失败。
"""

import json
import os
import re
import sys

def validate(settings: list[dict]) -> list[str]:
    errors: list[str] = []

    # 2) 必须有非空 i18n_key。
    for s in settings:
        if not s["i18n_key"].strip():
            errors.append(f"设置 {s['key']} 缺 i18n_key")

    # 3) onboarding.* 必须 device 作用域。
    for s in settings:
        if s["key"].startswith("onboarding.") and s["scope"] != "device":
            errors.append(f"onboarding 设置 {s['key']} 应为 device 作用域，实为 {s['scope']}")

    return errors


def validate_toggle_options(settings: list[dict], locales_dir: str | None) -> list[str]:
    """校验项 11：有序开关列表必须导出非空 `toggle_options`，且每个名字键在两种语言里存在。

    这种控件**不用** `option.<key>.<id>` 标签——它复用各对象已有的那一个名字（活动栏的七格
    就叫活动栏上的名字，PRD-07-003 / PRD-07-066「每个目的地只有一个名字键」）。故 7b 那条
    对它天然空跑；本条补上另一半：`label_key` 是 `ns:key`，去对应命名空间文件里找，找不到
    就是设置页里一整行裸 key。
    """
    errors: list[str] = []
    namespace_cache: dict[tuple[str, str], set[str]] = {}

    def namespace_keys(locale: str, namespace: str) -> set[str] | None:
        if locales_dir is None:
            return None
        cache_key = (locale, namespace)
        if cache_key not in namespace_cache:
            path = os.path.join(locales_dir, locale, f"{namespace}.json")
            if not os.path.exists(path):
                namespace_cache[cache_key] = set()
            else:
                with open(path, encoding="utf-8") as fh:
                    namespace_cache[cache_key] = flatten_keys(json.load(fh))
        return namespace_cache[cache_key]

    for s in settings:
        if s.get("control") != "ordered_toggle_list":
            continue
        options = s.get("toggle_options") or []
        if not options:
            errors.append(
                f"设置 {s['key']} 是 ordered_toggle_list 却没有导出 toggle_options"
                "——名字键校验会对它整项空跑"
            )
            continue
        ids = [o.get("id") for o in options]
        if len(set(ids)) != len(ids):
            errors.append(f"设置 {s['key']} 的 toggle_options 里有重复 id：{ids}")
        for option in options:
            label_key = option.get("label_key") or ""
            if ":" not in label_key:
                errors.append(
                    f"设置 {s['key']} 的选项「{option.get('id')}」名字键必须是 ns:key：{label_key!r}"
                )
                continue
            namespace, key = label_key.split(":", 1)
            for locale in ("zh-CN", "en-US"):
                keys = namespace_keys(locale, namespace)
                if keys is None:
                    continue
                if key not in keys:
                    errors.append(
                        f"设置 {s['key']} 的选项「{option.get('id')}」名字键缺失："
                        f"{locale}/{namespace}.json 无 {key}"
                    )
    return errors


def validate_ordered_multi_select(settings: list[dict]) -> list[str]:
    """校验项 10：有序多选必须真的导出选项与上下限。

    这一条守的是**另一条校验会不会空跑**。第 7b 条按 `options` 遍历检查选项 i18n；导出
    侧漏掉一种控件时，`options` 是空数组，7b 对它一条都不检查而全绿——五个创建动作 id
    的中英标签因此从未被校验过，少一条的表现是设置页下拉里蹦出裸取值 `text_quick_note`。

    `min`/`max` 同理：控件按它们决定"还能不能加/还能不能删"（TDD-05 §7.5a）。导出面上
    没有它们，前端就只能在自己那边写一份 2/4——而那份必然先陈旧。
    """
    errors: list[str] = []
    for s in settings:
        if s.get("control") != "ordered_multi_select":
            continue
        if not s.get("options"):
            errors.append(
                f"设置 {s['key']} 是 ordered_multi_select 却没有导出 options"
                "——选项 i18n 校验会对它整项空跑"
            )
        lower, upper = s.get("min"), s.get("max")
        if lower is None or upper is None:
            errors.append(f"设置 {s['key']} 缺 min/max（控件据此决定还能不能增删）")
        elif not 1 <= lower <= upper:
            errors.append(f"设置 {s['key']} 的 min/max 不成立：min={lower}, max={upper}")
    return errors


def frontend_groups(ia_source: str) -> set[str]:
    """解析前端 IA 的页面清单：`SETTINGS_PAGES = ["<页>", …] as const`。

    锚定到声明本体（`export const SETTINGS_PAGES`）再取方括号内的字符串字面量：
    文件头部注释也会提到页名，裸扫全文会把注释里的例子当成登记。
    """
    match = re.search(
        r"export const SETTINGS_PAGES\s*=\s*\[(.*?)\]\s*as const", ia_source, re.DOTALL
    )
    if not match:
        return set()
    return set(re.findall(r'"([a-z_]+)"', match.group(1)))


def validate_frontend_ia(settings: list[dict], ia_source: str) -> list[str]:
    """Rust 注册表里的每个页，前端 IA 清单里都必须在场。

    设置中心只呈现 `SETTINGS_PAGES` 里登记的页（`orderedPages` 按它过滤），于是 Rust
    新增一页而忘了在前端登记时，那一整页设置**在界面上根本不存在**——而 Rust 侧编译、
    前端 typecheck、既有测试没有任何一处会红。这条把它变成 CI 失败。
    """
    listed = frontend_groups(ia_source)
    errors: list[str] = []
    for group in sorted({s["group"] for s in settings}):
        if group not in listed:
            errors.append(
                f"页 {group} 在 Rust 注册表里有设置项，却不在前端 SETTINGS_PAGES 中"
                "——该页设置在设置中心里不会出现"
            )
    return errors


def panel_groups(ia_source: str) -> set[str]:
    """解析前端 `SETTINGS_PANEL_PAGES`（面板承载、零设置行也照常呈现的页）。"""
    # 锚定到声明本体（`export const`）：文件头部注释也会提到这个名字，裸名匹配会把
    # 注释后的第一个数组（SETTINGS_PAGES）误当成面板清单。
    match = re.search(
        r"export const SETTINGS_PANEL_PAGES[^=]*=\s*\[(.*?)\]", ia_source, re.DOTALL
    )
    if not match:
        return set()
    return set(re.findall(r'"([a-z_]+)"', match.group(1)))


def validate_panel_groups(
    settings: list[dict], ia_source: str, settings_view_source: str, app_source: str = ""
) -> list[str]:
    """校验项 8/9：零行页必须是面板型页，面板型页必须真的有面板。

    `orderedPages` 按"这一屏有没有设置行"过滤页。于是一页的全部键被标成内部/面板承载时，
    **任何投影下整页连同它的专属面板都不存在**——而 Rust 编译、前端 typecheck、前后端
    全部用例没有任何一处会红。模型页正是这样蒸发过一次：五个分配槽 + 两项参考单价全被
    标成面板承载，侧栏里从此没有「AI 算力与模型」。

    只在标准投影下隐藏（全部行为 advanced）的页是 TDD-05 §7.6 的刻意分层，不在此报错；
    "哪些面板型页必须在标准投影在场"由前端 IA 测试逐页钉住。
    """
    declared = panel_groups(ia_source)
    errors: list[str] = []
    for group in sorted({s["group"] for s in settings}):
        rows = [s for s in settings if s["group"] == group and s.get("surface") == "row"]
        if not rows and group not in declared:
            errors.append(
                f"页 {group} 的全部键都不渲染为设置行，却未登记进 SETTINGS_PANEL_PAGES"
                "——任何投影下该页连同其专属面板都不会出现在设置中心里"
            )
    # 两处登记面都算数：页内面板在 `SettingsView.PAGE_PANELS`，整页由外部 feature 承载的
    # 页（账户）在 `WorkspaceApp.SETTINGS_PAGE_PANELS`——跨 feature 组装归 `app/`
    # （TDD-15 §2.2），设置中心不反向依赖它。只认前者会把账户页判成"永远空白"。
    registered = group_panel_registrations(settings_view_source) | app_page_panels(app_source)
    for group in sorted(declared):
        if group not in registered:
            errors.append(
                f"页 {group} 声明为面板型，但 SettingsView 的 PAGE_PANELS 与 WorkspaceApp 的"
                " SETTINGS_PAGE_PANELS 里都没有它的面板——它会呈现为一个永远空白的页"
            )
    return errors


def after_section_keys(settings_view_source: str) -> dict[str, set[str]]:
    """解析 `PAGE_PANELS` 里各页的 `afterSection` 键：`<页> → {节 id}`。

    `afterSection` 让"属于某一节"的面板挂在那一节之后（一页多节时 `trail` 会把它甩到
    整页末尾，同一个概念于是在一页里出现两处）。键的类型是 `Record<string, …>`，写错
    一个字母 TypeScript 不会红，那个面板从此不进任何渲染分支。
    """
    block = re.search(r"const PAGE_PANELS[^=]*=\s*\{\n(.*?)\n\};", settings_view_source, re.DOTALL)
    if not block:
        return {}
    body = block.group(1)
    heads = list(re.finditer(r"^  ([a-z_]+): \{", body, re.MULTILINE))
    found: dict[str, set[str]] = {}
    for index, head in enumerate(heads):
        end = heads[index + 1].start() if index + 1 < len(heads) else len(body)
        entry = body[head.start():end]
        after = re.search(r"afterSection:\s*\{(.*?)\}", entry, re.DOTALL)
        if after:
            found[head.group(1)] = set(re.findall(r"([a-z_]+):", after.group(1)))
    return found


def validate_after_section(settings: list[dict], settings_view_source: str) -> list[str]:
    """`afterSection` 的每个键都必须是该页在注册表里真有的分节。"""
    declared: dict[str, set[str]] = {}
    for s in settings:
        if s.get("section"):
            declared.setdefault(s["group"], set()).add(s["section"])
    errors: list[str] = []
    for page, keys in sorted(after_section_keys(settings_view_source).items()):
        for key in sorted(keys):
            if key not in declared.get(page, set()):
                errors.append(
                    f"PAGE_PANELS.{page}.afterSection 的键「{key}」不是该页的分节"
                    "——挂在它上面的面板永远不会渲染"
                )
    return errors


# 分段控件一行排得下的**显示宽度**上限（中日韩字符记 2，西文记 1）。
#
# 不能按"字符数"量：中文 16 字与英文 16 字在同一列里差一倍宽。26 这个数由真机反推——
# 裂开过的两组分别是 27（英文 Small/Medium/Large/**Extra large**）与 43（中文
# 「Lunarca（暖调 + 月光金）」），当前排得下的最大一组是 25，故 26 恰好卡在中间。
SEGMENTED_WIDTH_BUDGET = 26


def display_width(text: str) -> int:
    """粗略显示宽度：中日韩字符占两格，其余一格。"""
    return sum(2 if ord(ch) > 0x2E80 else 1 for ch in text)


def validate_segmented_labels(
    settings: list[dict], registry_source: str, locales: dict[str, dict]
) -> list[str]:
    """分段控件的选项标签在**每一种语言**下都要排得下（PRD-09-070 ⑤b）。

    `SelectStyle` 的判据是「≤ 4 个**短**标签」，而"短"此前没有任何一处量过：
    「配色主题」的选项名里塞了「（暖调 + 月光金）」、英文字幕字号是 "Extra large"，
    两者都把控件挤成两排——headless 全绿，只有真机截图看得见。
    **判据认标签总宽，不认选项个数**：四个短标签排得下，三个里有一个是短语就排不下。
    """
    segmented = set()
    for block in re.findall(r"SettingSpec \{(.*?)\n    \},", registry_source, re.DOTALL):
        key = re.search(r'key: "([^"]+)"', block)
        if key and "SelectStyle::Segmented" in block:
            segmented.add(key.group(1))
    if not segmented:
        return ["注册表里一个分段控件都没有——本条判据恒真，等于这道门禁不存在"]
    errors: list[str] = []
    for setting in settings:
        if setting["key"] not in segmented:
            continue
        head, _, leaf = setting["key"].partition(".")
        for locale, bundle in locales.items():
            table = bundle.get("option", {}).get(head, {}).get(leaf, {})
            labels = [table.get(value, value) for value in setting.get("options", [])]
            width = sum(display_width(label) for label in labels)
            if width > SEGMENTED_WIDTH_BUDGET:
                errors.append(
                    f"{setting['key']} 是分段控件，但 {locale} 的选项标签合计宽 {width}"
                    f"（上限 {SEGMENTED_WIDTH_BUDGET}）：{labels}"
                    "——它会换行成两排，改短标签或在注册表里改用下拉"
                )
    return errors


# 字符宽度预算仅用于提示文案风险；实际行数必须通过渲染验收（ADR-132）。
STANDARD_DESC_WIDTH_BUDGET = 120
SIMPLE_SETTING_BUDGET = 29


def validate_simple_projection_budget(settings: list[dict]) -> list[str]:
    """简易设置的可见偏好必须维持已审阅的 29 项预算。"""
    simple_count = sum(setting.get("visibility") == "standard" for setting in settings)
    if simple_count == SIMPLE_SETTING_BUDGET:
        return []
    return [
        f"简易设置项数为 {simple_count}，应为 {SIMPLE_SETTING_BUDGET}；"
        "变更分层判据时须同步修改 PRD-09-082、ADR-130 与本门禁"
    ]


def validate_standard_desc_length(settings: list[dict], locales: dict[str, dict]) -> list[str]:
    """标准投影的说明字符预算（PRD-09-083 / ADR-132）。

    **只管标准投影**：高级项解释的正是简易层刻意藏起来的概念，压短等于把话说不清。
    判据认显示宽度而不是字符数——中文 60 字与英文 60 字在同一列里差一倍宽，而真机上
    先崩的从来是英文（同一句话英文长一倍）。
    """
    errors: list[str] = []
    flat = {locale: flatten_entries(tree) for locale, tree in locales.items()}
    for setting in settings:
        if setting.get("surface") != "row" or setting.get("visibility") != "standard":
            continue
        # 与 §7a 同一条派生（`desc_i18n_key`，去掉 `settings:` 前缀），不另猜一份 key。
        desc_key = setting.get("desc_i18n_key", f"{setting['i18n_key']}_desc").removeprefix(
            "settings:"
        )
        for locale, keys in flat.items():
            text = keys.get(desc_key)
            if not isinstance(text, str):
                continue
            width = display_width(text)
            if width > STANDARD_DESC_WIDTH_BUDGET:
                errors.append(
                    f"{setting['key']} 是标准项，但 {locale} 的说明宽 {width}"
                    f"（字符预算 {STANDARD_DESC_WIDTH_BUDGET}）；须缩短说明并检查实际渲染"
                )
    return errors


DESC_PRESENTATIONS = ("inline", "disclosure", "label_only")


def validate_desc_presentation(settings: list[dict]) -> list[str]:
    """设置说明的呈现方式由注册表逐键声明（PRD-09-084 / TDD-07 §4.1b，2026-10-08 D1）。

    每个渲染为设置行的键都必须带一个合法取值——缺失时前端只能自己猜（按长度、按前缀），
    那正是这条契约要杜绝的事。说明文案本身是否双语在场仍由 §7a 管：不常显的说明照样是
    搜索语料。
    """
    errors: list[str] = []
    for setting in settings:
        if setting.get("surface") != "row":
            continue
        value = setting.get("desc_presentation")
        if value not in DESC_PRESENTATIONS:
            errors.append(
                f"{setting['key']} 的 desc_presentation 为 {value!r}，"
                f"应为 {' / '.join(DESC_PRESENTATIONS)} 之一（由 Rust 注册表声明）"
            )
    return errors


def validate_aliases(locales: dict[str, dict]) -> list[str]:
    """同义词中英成对：`<i18n_key>_aliases` 与 `group_aliases.<页>`。

    别名是**匹配面**而不是显示文案，漏译不会渲染成裸 key、不会有任何一处变红——
    表现只是"同一个词在中文界面搜得到、切成英文就搜不到"，没人会想到去查它。
    """

    def alias_keys(bundle: dict) -> set[str]:
        found = {f"group_aliases.{page}" for page in bundle.get("group_aliases", {})}
        for head, node in bundle.items():
            if not isinstance(node, dict):
                continue
            found |= {f"{head}.{leaf}" for leaf in node if leaf.endswith("_aliases")}
        return found

    zh, en = alias_keys(locales["zh-CN"]), alias_keys(locales["en-US"])
    return [
        f"同义词 {key} 只在 {have} 有，{miss} 缺——搜索会在一种语言下悄悄失灵"
        for key, have, miss in (
            *((k, "zh-CN", "en-US") for k in sorted(zh - en)),
            *((k, "en-US", "zh-CN") for k in sorted(en - zh)),
        )
    ]


def app_page_panels(app_source: str) -> set[str]:
    """解析 `WorkspaceApp.tsx` 的 `SETTINGS_PAGE_PANELS`：条目 `<页>: <XxxPanel>,`。

    这里的值是**组件本身**而不是渲染函数（整页由它承载），故形状与 `PAGE_PANELS` 不同，
    单独解析而不是硬套同一条正则。
    """
    match = re.search(
        r"const SETTINGS_PAGE_PANELS[^=]*=\s*\{(.*?)\};", app_source, re.DOTALL
    )
    if not match:
        return set()
    return set(re.findall(r"^\s*([a-z_]+):\s*[A-Za-z]+,", match.group(1), re.MULTILINE))


def group_panel_registrations(settings_view_source: str) -> set[str]:
    """解析 `SettingsView.tsx` 的 `PAGE_PANELS` 登记面：条目 `<页>: { … <XxxPanel …> }`。

    与前端守护 `settings-panels.test.ts` 按同一形状解析：先切出对象字面量主体，再按两格
    缩进的条目头切块，块内出现 `…Panel` 组件的条目才算"真的有面板"（`ComputeCard` 是
    首屏答案卡、`OnboardingRerunRow` 是一行动作，都不是面板）。
    """
    block = re.search(r"const PAGE_PANELS[^=]*=\s*\{\n(.*?)\n\};", settings_view_source, re.DOTALL)
    if not block:
        return set()
    body = block.group(1)
    heads = list(re.finditer(r"^  ([a-z_]+): \{", body, re.MULTILINE))
    registered: set[str] = set()
    for index, head in enumerate(heads):
        end = heads[index + 1].start() if index + 1 < len(heads) else len(body)
        if re.search(r"<[A-Za-z]+Panel\b", body[head.start():end]):
            registered.add(head.group(1))
    return registered


def flatten_entries(tree: dict, prefix: str = "") -> dict[str, object]:
    """扁平化 locale JSON 为 `a.b.c` → 值。"""
    entries: dict[str, object] = {}
    for name, value in tree.items():
        path = f"{prefix}{name}"
        if isinstance(value, dict):
            entries |= flatten_entries(value, path + ".")
        else:
            entries[path] = value
    return entries


def flatten_keys(tree: dict, prefix: str = "") -> set[str]:
    """扁平化 locale JSON 为 `a.b.c` 键集合（`flatten_entries` 的投影，不另走一遍树）。"""
    return set(flatten_entries(tree, prefix))


def validate_i18n(
    settings: list[dict],
    locales: dict[str, dict],
    common_locales: dict[str, dict] | None = None,
) -> list[str]:
    """校验项 6/7：标签、说明、枚举选项标签在每个 locale 都在场。

    说明（`_desc`）对**内部状态**豁免（它们不渲染为可编辑行）；密钥项有标签与说明、
    但不校验取值。选项标签允许回落 `common:lang.*`（语言码的标签只该有一份）。
    """
    errors: list[str] = []
    flat = {locale: flatten_keys(tree) for locale, tree in locales.items()}
    common_flat = {
        locale: flatten_keys(tree) for locale, tree in (common_locales or {}).items()
    }
    for s in settings:
        internal = s.get("internal", False)
        i18n_key = s["i18n_key"]
        if not i18n_key.startswith("settings:"):
            if not internal:
                errors.append(f"设置 {s['key']} 的 i18n_key 不在 settings 命名空间：{i18n_key}")
            continue
        suffix = i18n_key.removeprefix("settings:")
        if not internal:
            for locale, keys in flat.items():
                if suffix not in keys:
                    errors.append(
                        f"设置 {s['key']} 的 i18n 标签缺失：{locale}/settings.json 无 {suffix}"
                    )
        if internal:
            continue
        # 7a) 一句说明（用户可见项必须能自我解释，密钥项同样要讲清密钥存哪儿）。
        desc_key = s.get("desc_i18n_key", f"{i18n_key}_desc").removeprefix("settings:")
        for locale, keys in flat.items():
            if desc_key not in keys:
                errors.append(
                    f"设置 {s['key']} 缺说明文案：{locale}/settings.json 无 {desc_key}"
                )
        # 7b) 枚举选项标签（否则下拉里出现裸取值）。
        for option in s.get("options", []):
            option_key = f"option.{s['key']}.{option}"
            for locale, keys in flat.items():
                if option_key in keys:
                    continue
                if f"lang.{option}" in common_flat.get(locale, set()):
                    continue
                errors.append(
                    f"设置 {s['key']} 的选项「{option}」缺标签：{locale} 无 {option_key}"
                )
        # 7c) 组内分节标题（PRD-09-070 ⑦）——否则分节标题渲染为裸 key。
        section = s.get("section")
        if section:
            section_key = f"section.{s['group']}.{section}"
            for locale, keys in flat.items():
                if section_key not in keys:
                    errors.append(
                        f"设置 {s['key']} 的分节「{section}」缺标题：{locale} 无 {section_key}"
                    )
    return errors


def run_self_test() -> int:
    base_settings = [
        {
            "key": "editor.autosave_enabled",
            "group": "notes",
            "scope": "vault",
            "i18n_key": "settings:editor.autosave_enabled",
        },
        {
            "key": "editor.autosave_debounce_ms",
            "group": "notes",
            "scope": "vault",
            "i18n_key": "settings:editor.autosave_debounce_ms",
        },
        {
            "key": "vault.snapshot_max_versions",
            "group": "notes",
            "scope": "vault",
            "i18n_key": "settings:vault.snapshot_max_versions",
        },
        {
            "key": "editor.floating_summary_lang",
            "group": "notes",
            "scope": "device",
            "i18n_key": "settings:editor.floating_summary_lang",
        },
        {
            "key": "onboarding.completed",
            "group": "general",
            "scope": "device",
            "i18n_key": "settings:onboarding.completed",
        },
    ]
    by_key = {setting["key"]: setting for setting in base_settings}
    cases = [
        ("valid", base_settings, False),
        (
            "missing i18n",
            [dict(base_settings[0], i18n_key="")],
            True,
        ),
        (
            "onboarding scope",
            [dict(by_key["onboarding.completed"], scope="vault")],
            True,
        ),
    ]

    failures: list[str] = []
    for name, settings, should_error in cases:
        errors = validate(settings)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 校验项 6 自测：标签齐全通过；缺一个 locale 报错；internal 豁免。
    locales_ok = {
        "zh-CN": {"editor": {"autosave_enabled": "自动保存", "autosave_enabled_desc": "说明"}},
        "en-US": {"editor": {"autosave_enabled": "Autosave", "autosave_enabled_desc": "Desc"}},
    }
    locales_missing_en = {
        "zh-CN": {"editor": {"autosave_enabled": "自动保存", "autosave_enabled_desc": "说明"}},
        "en-US": {},
    }
    locales_missing_desc = {
        "zh-CN": {"editor": {"autosave_enabled": "自动保存"}},
        "en-US": {"editor": {"autosave_enabled": "Autosave"}},
    }
    select_setting = dict(
        base_settings[3],
        options=["native", "zh"],
    )
    locales_select_ok = {
        "zh-CN": {
            "editor": {"floating_summary_lang": "总结语言", "floating_summary_lang_desc": "说明"},
            "option": {"editor.floating_summary_lang": {"native": "母语"}},
        },
        "en-US": {
            "editor": {"floating_summary_lang": "Summary language", "floating_summary_lang_desc": "Desc"},
            "option": {"editor.floating_summary_lang": {"native": "Native"}},
        },
    }
    common_ok = {"zh-CN": {"lang": {"zh": "中文"}}, "en-US": {"lang": {"zh": "Chinese"}}}
    i18n_cases = [
        ("i18n complete", base_settings[:1], locales_ok, None, False),
        ("i18n missing in en", base_settings[:1], locales_missing_en, None, True),
        ("desc missing", base_settings[:1], locales_missing_desc, None, True),
        (
            "internal exempt from i18n",
            [dict(base_settings[0], internal=True)],
            locales_missing_en,
            None,
            False,
        ),
        ("select options labelled", [select_setting], locales_select_ok, common_ok, False),
        ("select option label missing", [select_setting], locales_select_ok, None, True),
        # 校验项 7c 自测：分节标题双语在场通过；缺失即红（PRD-09-070 ⑦）。
        (
            "section title labelled",
            [dict(base_settings[0], section="privacy")],
            {
                locale: {
                    **tree,
                    "section": {"notes": {"privacy": "隐私"}},
                }
                for locale, tree in locales_ok.items()
            },
            None,
            False,
        ),
        (
            "section title missing",
            [dict(base_settings[0], section="privacy")],
            locales_ok,
            None,
            True,
        ),
    ]
    for name, settings, locales, common, should_error in i18n_cases:
        errors = validate_i18n(settings, locales, common)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 前端 IA 自测：页面清单从 `SETTINGS_PAGES` 的声明本体解析，注释里提到的名字不算登记。
    ia_with_group = (
        "/** 例：`recording` 是录音与转写页。 */\n"
        "export const SETTINGS_PAGES = [\n"
        '  "general",\n  "shortcuts",\n  "recording",\n'
        "] as const;"
    )
    ia_missing_group = (
        "/** 例：`recording` 是录音与转写页。 */\n"
        "export const SETTINGS_PAGES = [\n"
        '  "general",\n  "shortcuts",\n'
        "] as const;"
    )
    recording_row = {
        "key": "recording.format",
        "group": "recording",
        "scope": "device",
        "i18n_key": "settings:recording.format",
        "visibility": "standard",
        "surface": "row",
    }
    for name, ia, should_error in [
        ("frontend ia lists the page", ia_with_group, False),
        ("frontend ia misses the page (comment mention does not count)", ia_missing_group, True),
    ]:
        errors = validate_frontend_ia([recording_row], ia)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 校验项 8/9 自测：模型页整页蒸发那一幕的最小复现（全部键面板承载 + 未登记面板型）。
    hidden_row = {
        "key": "ai.assignment.default_llm",
        "group": "local_models",
        "scope": "device",
        "i18n_key": "settings:ai.assignment.default_llm",
        "visibility": "standard",
        "surface": "panel",
    }
    advanced_row = dict(
        hidden_row,
        key="provider.asr_cost_per_min",
        i18n_key="settings:provider.asr_cost_per_min",
        visibility="advanced",
        surface="row",
    )
    standard_row = dict(advanced_row, visibility="standard")
    ia_without_panel = 'export const SETTINGS_PANEL_PAGES: readonly X[] = ["glossary"];'
    ia_with_panel = (
        'export const SETTINGS_PANEL_PAGES: readonly X[] = ["glossary", "local_models"];'
    )
    view_with_branch = (
        "const PAGE_PANELS: Partial<Record<SettingsPageId, PagePanels>> = {\n"
        "  glossary: {\n    trail: () => <GlossaryPanel />,\n  },\n"
        "  local_models: {\n    lead: () => <ComputeCard />,\n"
        "    trail: () => (\n      <ModelManagementPanel />\n    ),\n  },\n"
        "};"
    )
    view_glossary_only = (
        "const PAGE_PANELS: Partial<Record<SettingsPageId, PagePanels>> = {\n"
        "  glossary: {\n    trail: () => <GlossaryPanel />,\n  },\n"
        "  local_models: {\n    lead: () => <ComputeCard />,\n  },\n"
        "};"
    )
    panel_cases = [
        # 全部面板承载 + 未登记 → 任何投影下整页消失，必须红。
        ("panel page unregistered", [hidden_row], ia_without_panel, view_glossary_only, True),
        # 只剩高级行 = TDD-05 §7.6 的刻意分层（完整投影下在场），不报错。
        (
            "advanced-only page is deliberate tiering",
            [advanced_row],
            ia_without_panel,
            view_glossary_only,
            False,
        ),
        # 登记为面板型且面板真的存在 → 通过。
        ("panel page registered", [hidden_row], ia_with_panel, view_with_branch, False),
        # 登记了却没有任何渲染分支 → 永远空白的页，必须红。
        ("panel page without panel", [hidden_row], ia_with_panel, view_glossary_only, True),
        # 账户页整页由 `WorkspaceApp.SETTINGS_PAGE_PANELS` 承载：只认 SettingsView 会把它
        # 误判成"永远空白的页"，而它恰恰是全设置中心最不能消失的一页。
        # 有标准设置行的普通页不需要登记。
        (
            "plain group needs no exemption",
            [standard_row],
            ia_without_panel,
            view_glossary_only,
            False,
        ),
    ]
    for name, rows, ia, view, should_error in panel_cases:
        errors = validate_panel_groups(rows, ia, view)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 标准项说明的两行上限：只管标准投影，高级项不设限（它解释的正是简易层藏起来的概念）。
    desc_setting = {
        "key": "privacy.strict_local",
        "i18n_key": "settings:privacy.strict_local",
        "desc_i18n_key": "settings:privacy.strict_local_desc",
        "surface": "row",
        "visibility": "standard",
    }
    short_desc = {loc: {"privacy": {"strict_local_desc": "开启后不再发起任何出站联网。"}}
                  for loc in ("zh-CN", "en-US")}
    long_desc = {
        "zh-CN": {"privacy": {"strict_local_desc": "开" * 80}},
        "en-US": short_desc["en-US"],
    }
    for name, bundles, setting, should_error in [
        ("standard desc fits two lines", short_desc, desc_setting, False),
        ("standard desc overflows", long_desc, desc_setting, True),
        # 同一句话挂在高级项上不报错——判据只管标准投影。
        ("advanced desc is exempt", long_desc, dict(desc_setting, visibility="advanced"), False),
    ]:
        errors = validate_standard_desc_length([setting], bundles)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 说明呈现方式：设置行必须带合法取值；非设置行不管（面板承载与内部键没有行可呈现）。
    for name, row, should_error in [
        ("row declares inline", dict(desc_setting, desc_presentation="inline"), False),
        ("row declares label_only", dict(desc_setting, desc_presentation="label_only"), False),
        ("row missing presentation", desc_setting, True),
        ("row with unknown presentation", dict(desc_setting, desc_presentation="hover"), True),
        ("panel-owned key is exempt", dict(desc_setting, surface="panel"), False),
    ]:
        errors = validate_desc_presentation([row])
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    simple_rows = [dict(desc_setting, key=f"simple.{index}") for index in range(SIMPLE_SETTING_BUDGET)]
    if validate_simple_projection_budget(simple_rows):
        failures.append("simple setting budget accepts the reviewed count")
    if not validate_simple_projection_budget(simple_rows[:-1]):
        failures.append("simple setting budget rejects a drifted count")

    # 分段控件的标签预算：中文排得下、英文裂开是常态，故两套口径各自校验。
    seg_src = 'SettingSpec {\n        key: "appearance.theme_family",\n        style: SelectStyle::Segmented,\n    },'
    seg_setting = {"key": "appearance.theme_family", "options": ["luna", "nebula"]}
    short = {loc: {"option": {"appearance": {"theme_family": {"luna": "Lunarca", "nebula": "Nebula"}}}}
             for loc in ("zh-CN", "en-US")}
    longish = {
        "zh-CN": {"option": {"appearance": {"theme_family": {"luna": "Lunarca（暖调 + 月光金）", "nebula": "星云（中性灰 + 紫）"}}}},
        "en-US": short["en-US"],
    }
    wide_en = {
        "zh-CN": short["zh-CN"],
        "en-US": {"option": {"appearance": {"theme_family": {"luna": "Small Medium Large", "nebula": "Extra large"}}}},
    }
    for name, bundles, should_error in [
        ("segmented labels fit", short, False),
        ("segmented labels overflow in zh", longish, True),
        ("segmented labels overflow in en only", wide_en, True),
    ]:
        errors = validate_segmented_labels([seg_setting], seg_src, bundles)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")
    # 判据自身不空跑：注册表里没有分段控件时必须报错。
    if not validate_segmented_labels([seg_setting], "", short):
        failures.append("segmented guard runs empty when no segmented control exists")

    # 同义词中英成对：只译一半时搜索会在一种语言下悄悄失灵。
    paired = {
        "zh-CN": {"group_aliases": {"glossary": "词典"}, "appearance": {"theme_aliases": "深色"}},
        "en-US": {"group_aliases": {"glossary": "dictionary"}, "appearance": {"theme_aliases": "dark"}},
    }
    half = {
        "zh-CN": {"group_aliases": {"glossary": "词典"}, "appearance": {"theme_aliases": "深色"}},
        "en-US": {"group_aliases": {}, "appearance": {"theme_aliases": "dark"}},
    }
    for name, bundles, should_error in [
        ("aliases paired", paired, False),
        ("aliases half translated", half, True),
    ]:
        errors = validate_aliases(bundles)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 面板承载的键不得配一个自造的原生控件（ADR-122 ⑤）。判据要同时抓住两件事：
    # 承载了那个键、且自造了控件——只占其一都不该红。
    panel_row = {
        "key": "provider.remote_asr_enabled",
        "group": "ai_services",
        "scope": "device",
        "i18n_key": "settings:provider.remote_asr_enabled",
        "visibility": "standard",
        # 形状逐字取自 `target/settings-registry.json`（字符串，不是 TS 侧那个联合类型的
        # 对象形式）——自测编一个形状出来，测的就是一件线上不存在的事。
        "surface": "panel",
        "section": "compute",
    }
    row_row = dict(panel_row, key="ai.route_strategy", surface="row")
    for name, rows, sources, should_error in [
        (
            "panel key with a hand-rolled checkbox",
            [panel_row],
            {"a.tsx": 'const K = "provider.remote_asr_enabled";\n<input type="checkbox" />'},
            True,
        ),
        (
            "panel key going through SettingControl",
            [panel_row],
            {"a.tsx": 'const K = "provider.remote_asr_enabled";\n<SettingControl spec={s} />'},
            False,
        ),
        (
            # 与设置项无关的勾选框照常放过——本仓多处用它做一次性选择。
            "a checkbox with no registry key in sight",
            [panel_row],
            {"a.tsx": '<input type="checkbox" checked={includeVault} />'},
            False,
        ),
        (
            # 行承载的键不归这条判据：它本来就由 SettingsView 渲染。
            "row key is not this rule's business",
            [panel_row, row_row],
            {"a.tsx": 'const K = "ai.route_strategy";\n<input type="checkbox" />'},
            False,
        ),
        (
            # 注释里提到那个控件是**解释**，不是违规。这条判据的第一版正是栽在这里：
            # 承载该键的文件在头注里写着它此前为什么错，于是门禁把自己判红了。
            "a comment explaining the old defect is not a defect",
            [panel_row],
            {
                "a.tsx": (
                    '// 此前手写了 <input type="checkbox" />\n'
                    'const K = "provider.remote_asr_enabled";\n<SettingControl />'
                )
            },
            False,
        ),
        (
            # URL 里的 `//` 不得被当成行注释吃掉后半截——那半截里正好有 key。
            "a settings:// url is not a line comment",
            [panel_row],
            {
                # 同一行里先 URL 后 key：把 `//` 当行注释的实现会连 key 一起吃掉，
                # 于是这条违规静默放过——夹具正是要区分这两种实现。
                "a.tsx": (
                    'const R = "settings://x"; const K = "provider.remote_asr_enabled";\n'
                    '<input type="checkbox" />'
                )
            },
            True,
        ),
        (
            # 判据不许空转：一个面板承载键都认不出来时必须自己报红，而不是恒绿。
            "no panel keys at all means the rule went blind",
            [row_row],
            {"a.tsx": "<div />"},
            True,
        ),
    ]:
        errors = validate_panel_controls(rows, sources)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # `afterSection` 的键必须是真分节：写错一个字母，那个面板一个渲染分支都不进。
    after_view = (
        "const PAGE_PANELS: Partial<Record<SettingsPageId, PagePanels>> = {\n"
        "  external_tools: {\n    afterSection: { tools: () => <ExternalToolsPanel /> },\n  },\n"
        "};"
    )
    tools_row = {
        "key": "transcribe.fetch_proxy",
        "group": "external_tools",
        "scope": "device",
        "i18n_key": "settings:transcribe.fetch_proxy",
        "visibility": "advanced",
        "surface": "row",
        "section": "tools",
    }
    for name, rows, should_error in [
        ("afterSection key is a real section", [tools_row], False),
        ("afterSection key is a typo", [dict(tools_row, section="tool")], True),
    ]:
        errors = validate_after_section(rows, after_view)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 账户页整页由 `WorkspaceApp.SETTINGS_PAGE_PANELS` 承载：只认 SettingsView 的登记面会把
    # 它误判成"永远空白的页"，而它恰恰是全设置中心最不能消失的一页。
    account_ia = 'export const SETTINGS_PANEL_PAGES: readonly X[] = ["account"];'
    account_app = (
        "const SETTINGS_PAGE_PANELS: SettingsPagePanels = {\n  account: AccountPanel,\n};"
    )
    for name, app, should_error in [
        ("account page carried by app", account_app, False),
        ("account page carried by nobody", "", True),
    ]:
        errors = validate_panel_groups([], account_ia, view_glossary_only, app)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 校验项 10 自测：导出侧漏掉一种控件（options 空 / 缺 min-max）必须红。
    ordered_ok = {
        "key": "workspace.quick_start_actions",
        "group": "workspace",
        "scope": "device",
        "i18n_key": "settings:workspace.quick_start_actions",
        "control": "ordered_multi_select",
        "options": ["text_quick_note", "import_material"],
        "min": 2,
        "max": 4,
    }
    ordered_cases = [
        ("ordered multi select fully exported", [ordered_ok], False),
        # 这就是本批修掉的那一幕：`_ => Vec::new()` 把它变成空数组，选项 i18n 校验随之空跑。
        ("ordered multi select without options", [dict(ordered_ok, options=[])], True),
        (
            "ordered multi select without bounds",
            [{k: v for k, v in ordered_ok.items() if k not in ("min", "max")}],
            True,
        ),
        ("ordered multi select with inverted bounds", [dict(ordered_ok, min=4, max=2)], True),
        # 其它控件不受这条约束（`Toggle` 本来就没有选项与区间）。
        (
            "other controls are untouched",
            [dict(base_settings[0], control="toggle", options=[])],
            False,
        ),
    ]
    for name, rows, should_error in ordered_cases:
        errors = validate_ordered_multi_select(rows)
        if bool(errors) != should_error:
            failures.append(f"{name}: expected error={should_error}, got {errors}")

    # 校验项 11 自测：有序开关列表的名字键必须是 ns:key、且在两种语言的命名空间里都存在。
    import tempfile

    with tempfile.TemporaryDirectory() as locales_dir:
        for locale in ("zh-CN", "en-US"):
            os.makedirs(os.path.join(locales_dir, locale))
            with open(
                os.path.join(locales_dir, locale, "workspace.json"), "w", encoding="utf-8"
            ) as fh:
                json.dump({"nav": {"notes": "笔记"}}, fh)
        toggle_ok = {
            "key": "workspace.rail_layout",
            "control": "ordered_toggle_list",
            "toggle_options": [{"id": "notes", "label_key": "workspace:nav.notes", "locked": True}],
        }
        toggle_cases = [
            ("toggle list fully exported", [toggle_ok], False),
            ("toggle list without options", [dict(toggle_ok, toggle_options=[])], True),
            (
                "toggle list label key without namespace",
                [dict(toggle_ok, toggle_options=[{"id": "notes", "label_key": "nav.notes"}])],
                True,
            ),
            (
                "toggle list label key missing in locale",
                [dict(toggle_ok, toggle_options=[{"id": "x", "label_key": "workspace:nav.nope"}])],
                True,
            ),
            (
                "toggle list duplicate ids",
                [
                    dict(
                        toggle_ok,
                        toggle_options=[
                            {"id": "notes", "label_key": "workspace:nav.notes"},
                            {"id": "notes", "label_key": "workspace:nav.notes"},
                        ],
                    )
                ],
                True,
            ),
            ("other controls are untouched", [dict(base_settings[0], control="toggle")], False),
        ]
        for name, rows, should_error in toggle_cases:
            errors = validate_toggle_options(rows, locales_dir)
            if bool(errors) != should_error:
                failures.append(f"{name}: expected error={should_error}, got {errors}")

    if failures:
        print("SELF-TEST FAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("SELF-TEST PASS")
    return 0


def stale_snapshot_reason(registry_path: str) -> str | None:
    """快照是不是比它的来源还旧。

    生成与校验是**分开两行**（AGENTS.md §3）：
        cargo run -p luna-app --bin export-settings-registry > target/settings-registry.json
        python3 scripts/check-settings-registry.py target/settings-registry.json …
    漏掉第一行时第二行**照样通过**——它验的是上一次留下的快照，而那份快照可能已经与
    注册表差了好几批改动。这不是假想：2026-08-08 就发生过一次，连续几批的「校验通过」
    验的都是陈旧数据，而没有任何东西会因此变红。

    故这里自己比一次时间戳。不在脚本里直接跑 cargo——那会让一个纯文本校验脚本变成
    需要完整 Rust 工具链才能运行的东西。
    """
    if not os.path.exists(registry_path):
        return f"{registry_path} 不存在"
    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    source = os.path.join(repo_root, "crates/luna-config/src/registry.rs")
    if not os.path.exists(source):
        return None
    snapshot_mtime = os.path.getmtime(registry_path)
    if snapshot_mtime < os.path.getmtime(source):
        return f"{os.path.basename(registry_path)} 比 registry.rs 旧"
    return None



def frontend_tsx_sources(repo_root: str) -> dict[str, str]:
    """`apps/desktop/src/features` 下的全部 `.tsx`（测试与故事除外）。"""
    base = os.path.join(repo_root, "apps/desktop/src/features")
    out: dict[str, str] = {}
    for dirpath, _dirs, files in os.walk(base):
        for name in files:
            if not name.endswith(".tsx") or ".test." in name:
                continue
            path = os.path.join(dirpath, name)
            with open(path, encoding="utf-8") as fh:
                out[os.path.relpath(path, repo_root)] = fh.read()
    return out


# 面板里自造的原生控件。`<select` 不入判据：本仓多处用它做**非设置**的即时选择
# （导出格式、筛选档），把它们一并判红只会逼出一堆豁免。勾选框不同——设置族的布尔
# 一律是 `role="switch"` 的开关，一个原生勾选框出现在承载注册表键的文件里，必然是
# 绕过了注册表声明的那条路。
NATIVE_CONTROL_MARKERS = ('type="checkbox"',)


def strip_ts_comments(source: str) -> str:
    """剥掉 TS/TSX 的注释，保留字符串字面量。

    判据必须读**代码**而不是全文：这条门禁的第一版栽在自己身上——`TranscriptionConsentRow`
    的头注里写着 `type="checkbox"`（在解释它此前为什么错），于是基线当场红。同一类问题
    在本仓出现过（守护读全文会被注释里的类名喂饱）。

    走一遍字符状态机而不是正则：`settings://provider_model/...` 这种 URL 里有 `//`，
    正则版会把那一行的后半截当注释吃掉，而那半截里正好有 key。
    """
    out: list[str] = []
    i, n = 0, len(source)
    quote: str | None = None
    while i < n:
        ch = source[i]
        if quote is not None:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(source[i + 1])
                i += 2
                continue
            if ch == quote:
                quote = None
            i += 1
            continue
        if ch in "\"'`":
            quote = ch
            out.append(ch)
            i += 1
            continue
        if source.startswith("//", i):
            while i < n and source[i] != "\n":
                i += 1
            continue
        if source.startswith("/*", i):
            end = source.find("*/", i + 2)
            i = n if end == -1 else end + 2
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def validate_panel_controls(settings: list[dict], sources: dict[str, str]) -> list[str]:
    """面板承载的键必须走共享控件渲染器（ADR-122 ⑤）。

    判据：一个文件里**同时**出现 (a) 某个面板承载键的字面量、(b) 自造的原生控件 —— 即红。
    键清单取自注册表导出，不在这里维护第二份（维护第二份必然与注册表分叉，而分叉时
    没有任何东西会变红——这正是转写同意那一处的成因：注册表声明 `Toggle`，承载它的
    算力卡手写了原生勾选框，全应用唯一一个原生勾选框正好落在那条隐私同意上）。
    """
    # `surface` 在导出物里是**字符串**（`row` / `panel` / `internal`）。初版按 TS 侧的
    # 联合类型写成了 dict 判断，于是这张清单恒为空、门禁恒绿——而自测自己编了 dict 形状，
    # 连自测都没发现。判据下面那条「不许空转」就是为这件事加的。
    panel_keys = [spec["key"] for spec in settings if spec.get("surface") == "panel"]
    errors: list[str] = []
    if not panel_keys:
        return ["注册表里一个面板承载键都没有——判据已空转（多半是 `surface` 的形状变了）"]
    for path, raw in sorted(sources.items()):
        source = strip_ts_comments(raw)
        hit = next((marker for marker in NATIVE_CONTROL_MARKERS if marker in source), None)
        if hit is None:
            continue
        for key in panel_keys:
            if f'"{key}"' in source:
                errors.append(
                    f"{path} 承载注册表键 {key}，却自造了 {hit}；"
                    "面板承载的键必须走 SettingControl（ADR-122 ⑤）"
                )
    return errors


def main() -> int:
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        return run_self_test()

    if len(sys.argv) not in (2, 3):
        print(__doc__)
        return 2

    registry_path = sys.argv[1]
    locales_arg = sys.argv[2] if len(sys.argv) == 3 else None
    if stale := stale_snapshot_reason(registry_path):
        print(f"注册表快照已过期：{stale}")
        print("  重新生成：cargo run -p luna-app --bin export-settings-registry "
              "> target/settings-registry.json")
        return 1
    with open(registry_path, encoding="utf-8") as fh:
        settings = json.load(fh)
    errors = validate(settings)

    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    ia_path = os.path.join(
        repo_root,
        "apps/desktop/src/features/settings/settings-information-architecture.ts",
    )
    settings_view_path = os.path.join(
        repo_root, "apps/desktop/src/features/settings/SettingsView.tsx"
    )
    app_path = os.path.join(repo_root, "apps/desktop/src/app/WorkspaceApp.tsx")
    with open(ia_path, encoding="utf-8") as fh:
        ia_source = fh.read()
    if 'import { SETTINGS_PAGES } from "../../ipc/bindings"' in ia_source:
        with open(os.path.join(repo_root, "apps/desktop/src/ipc/bindings.ts"), encoding="utf-8") as fh:
            generated = fh.read()
        declaration = re.search(
            r"export const SETTINGS_PAGES\s*=\s*\[.*?\]\s*as const", generated, re.DOTALL
        )
        if declaration:
            ia_source += "\n" + declaration.group(0)
    with open(settings_view_path, encoding="utf-8") as fh:
        settings_view_source = fh.read()
    with open(app_path, encoding="utf-8") as fh:
        app_source = fh.read()
    errors += validate_frontend_ia(settings, ia_source)
    errors += validate_panel_groups(settings, ia_source, settings_view_source, app_source)
    errors += validate_after_section(settings, settings_view_source)
    errors += validate_panel_controls(settings, frontend_tsx_sources(repo_root))
    errors += validate_ordered_multi_select(settings)
    errors += validate_simple_projection_budget(settings)
    errors += validate_desc_presentation(settings)
    errors += validate_toggle_options(settings, locales_arg)

    if locales_arg is not None:
        locales_dir = locales_arg
        locales: dict[str, dict] = {}
        common_locales: dict[str, dict] = {}
        for locale in ("zh-CN", "en-US"):
            path = os.path.join(locales_dir, locale, "settings.json")
            with open(path, encoding="utf-8") as fh:
                locales[locale] = json.load(fh)
            common_path = os.path.join(locales_dir, locale, "common.json")
            with open(common_path, encoding="utf-8") as fh:
                common_locales[locale] = json.load(fh)
        errors += validate_i18n(settings, locales, common_locales)
        errors += validate_aliases(locales)
        errors += validate_standard_desc_length(settings, locales)
        registry_source_path = os.path.join(repo_root, "crates/luna-config/src/registry.rs")
        with open(registry_source_path, encoding="utf-8") as fh:
            errors += validate_segmented_labels(settings, fh.read(), locales)
    if errors:
        print("设置注册表一致性校验失败：")
        for err in errors:
            print(f"  - {err}")
        return 1

    present_groups = sorted({s["group"] for s in settings})
    print(f"PRD-09 设置一致性校验通过（{len(settings)} 项，分组 {len(present_groups)} 个）。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
