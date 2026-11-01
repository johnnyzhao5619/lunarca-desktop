/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import chatViewRaw from "../features/chat/ChatView.tsx?raw";
import captionPanelRaw from "../features/caption/CaptionPanel.tsx?raw";
import popupResultRaw from "../features/popup-translate/PopupResultSlot.tsx?raw";
import popupTranslateRaw from "../features/popup-translate/PopupTranslateView.tsx?raw";
import recordingHudRaw from "../features/recording/RecordingHud.tsx?raw";
import voiceHudRaw from "../features/voice-input/VoiceInputHud.tsx?raw";
import { runtimeStyleCss as cssRaw } from "./style-source";
import chatEnRaw from "../i18n/locales/en-US/chat.json?raw";
import chatZhRaw from "../i18n/locales/zh-CN/chat.json?raw";
import popupEnRaw from "../i18n/locales/en-US/popup.json?raw";
import popupZhRaw from "../i18n/locales/zh-CN/popup.json?raw";
import voiceEnRaw from "../i18n/locales/en-US/voice-input.json?raw";
import voiceZhRaw from "../i18n/locales/zh-CN/voice-input.json?raw";
import hudSurfaceRaw from "../components/ui/HudSurface.tsx?raw";

/**
 * HUD 信息密度守护（2026-07-27 真机截图回归）。
 *
 * 这些断言锁的不是像素截图，而是五类 HUD 共用的操作层级：正文优先、一个当前主动作、
 * 图标化的邻近输入工具，以及只在有结果后出现的结果动作。业务状态仍由各 feature / Rust
 * 控制器拥有；本测试只防止呈现层重新堆回常驻按钮和大块空白。
 */
describe("HUD 信息密度与操作层级", () => {
  it("快捷对话把低频会话动作收进菜单，空态预设保持双列紧凑", () => {
    expect(chatViewRaw).toContain("<HudActionMenu");
    expect(chatViewRaw).toContain('label={t("action.more")}');
    // ADR-137 H7：「在主工作区继续」进菜单；模型是状态行里的纯文字；消息悬停只露复制 + ⋯；
    // 输入框按镜像格子长高，不在 JS 里写内联高度。
    expect(chatViewRaw).toMatch(/id: "continue",\s*label: t\("action\.continue_workspace"\)/);
    expect(chatViewRaw).toContain("lane={modelBadge}");
    expect(chatViewRaw).toContain('density="sm"');
    expect(chatViewRaw).not.toMatch(/\.style\.height\s*=/);
    expect(chatViewRaw).toContain('className="luna-chat__composer-grow"');
    expect(cssRaw).toMatch(
      /\.luna-chat\[data-panel="true"\] \.luna-chat__preset-chips\s*\{[^}]*grid-template-columns:\s*repeat\(2,/s,
    );
    // 空态外壳已收敛到 `.luna-empty-state` 原语（此前是对话自己的第四套实现）；
    // 本断言锁的始终是**面板形态里空态贴底**这条呈现契约，与它挂在哪个类上无关。
    expect(cssRaw).toMatch(
      /\.luna-chat\[data-panel="true"\] \.luna-empty-state\s*\{[^}]*justify-content:\s*flex-end/s,
    );
  });

  it("快捷对话空态与默认自动保存契约一致，不再引用已删除的自动归档", () => {
    // 2026-10-08（C33）：「自动保存」只说一次，落在历史列表的空态上——那是它被看见的地方；
    // 对话空态只请用户开口，临时对话由页头徽标常显「不保存」。
    expect(chatZhRaw).toContain("对话会自动保存到这里");
    expect(chatEnRaw).toContain("Chats are saved here automatically");
    expect(chatZhRaw).not.toContain("仅保存在内存");
    expect(chatEnRaw).not.toContain("live in memory only");
    expect(`${chatZhRaw}${chatEnRaw}`).not.toMatch(/自动归档|auto-archive/i);
  });

  it("翻译输入只保留一个文字主动作，空结果不陈列不可用动作", () => {
    // 图标钮已收编为 `IconButton` 原语（名字必填、密度走 `density`），故断言锁的是
    // **顺序契约**：输入行里两枚 HUD 档图标动作（拍照、语音）在前，唯一的文字主动作在后。
    expect(popupTranslateRaw).toMatch(
      /luna-popup__input-row[\s\S]*?<IconButton[\s\S]*?density="hud"[\s\S]*?<CameraIcon[\s\S]*?<IconButton[\s\S]*?density="hud"[\s\S]*?(?:MicIcon|StopIcon)[\s\S]*?luna-popup__generate/,
    );
    expect(popupResultRaw).toContain('slot.status !== "idle"');
    // 主动作皮归原语（ADR-111 ② / 批 B）：按钮显式挂 `.luna-btn--primary`，feature 类
    // 只留 HUD 密度差异——不再断言 feature 层写主操作色（那正是要禁的事，§3.3e）。
    // 有结果后生成退为次级：同屏只留一颗主色按钮（复制 / 替换），ADR-137 V3。
    expect(popupTranslateRaw).toContain(
      'className={`luna-btn ${resultsVisible ? "luna-btn--secondary" : "luna-btn--primary"} luna-popup__generate`}',
    );
    expect(popupTranslateRaw).toMatch(
      /<IconButton[\s\S]*?density="hud"[\s\S]*?label=\{t\("action\.add_compare"\)\}[\s\S]*?<PlusIcon/,
    );
    expect(popupTranslateRaw).not.toContain('className="luna-popup__compare"');
    // ADR-112 ④：结果区首 token 才存在——输入态只有输入卡，不画空的「结果」框与预设下拉。
    expect(popupTranslateRaw).toMatch(/\{resultsVisible && \(\s*<div className="luna-popup__slots"/);
    // 空输入禁用主动作；生成中主动作原位变「停止」（中断一律 --danger）。
    expect(popupTranslateRaw).toContain('disabled={popupIn.trim() === ""}');
    expect(popupTranslateRaw).toContain('className="luna-btn luna-btn--danger luna-popup__generate"');
    // 预设分组由 Rust 下发的 group 决定，前端不再按 id 前缀猜（ADR-003）。
    expect(popupResultRaw).not.toContain('startsWith("to_")');
    expect(cssRaw).toMatch(
      /\.luna-popup-slot > \.luna-task-status\s*\{[^}]*align-self:\s*flex-start/s,
    );
    expect(cssRaw).toMatch(
      /\.luna-result-actions__primary\[data-copied="true"\]\s*\{[^}]*background:\s*var\(--color-status-success-surface\)/s,
    );
    expect(popupZhRaw).toContain("正在生成译文");
    expect(popupEnRaw).toContain("Generating translation");
  });

  it("语音与录音以当前模式和当前主动作替代重复说明", () => {
    // 窗口栏归 HudHeader（批 C）：标题/胶囊皮在原语里只有一份；这里锁「模式走
    // 配置胶囊槽」这条语义（context=当前模式），不再锁类名字面。
    expect(popupTranslateRaw).toContain("<HudHeader");
    expect(voiceHudRaw).toContain("<HudHeader");
    // 语音的状态与提示和状态行同排（ADR-137 V2）：一行就是整个胶囊。
    expect(voiceHudRaw).toContain("lane={lane}");
    expect(voiceHudRaw).not.toContain("luna-voice__mode-badge");
    expect(voiceHudRaw).not.toContain('className="luna-voice__tool-btn"');
    expect(recordingHudRaw).toContain("<MicIcon size={15} />");
    expect(recordingHudRaw).not.toContain('className="luna-rec-hud__context"');
    expect(cssRaw).toContain("--hud-content-min-block-size: 136px;");
    // 「完成」是停止采集，用中性强调而非危险色（ADR-137 V3：红色只留给录音指示与不可撤销的
    // 放弃）——圆钮按 data-kind 出形，样式里 stop 态不得取错误色。
    expect(voiceHudRaw).toContain("data-kind={primaryAction.kind}");
    expect(voiceHudRaw).not.toContain("luna-btn--danger luna-voice__primary");
    expect(cssRaw).toMatch(/\.luna-voice__orb\[data-kind="stop"\]\s*\{[^}]*background:\s*var\(--color-text-primary\)/s);
  });

  it("语音输入空态不承诺自动写入，保持先审阅再落地的业务事实", () => {
    // 空闲胶囊的落点胶囊提示里写着承诺（ADR-137：空闲说明句退役，承诺随落点一起出现）。
    expect(voiceZhRaw).toContain("识别后先审阅，确认后才写入");
    expect(voiceEnRaw).toContain("You review the result before it goes to");
    expect(voiceHudRaw).toContain('title={t("landing.review_first"');
    expect(voiceZhRaw).not.toContain("自动识别、优化并写入目标位置");
    expect(voiceEnRaw).not.toContain("write to the selected destination");
  });

  it("实时字幕顶栏复用 HUD 图标控制，不在窄窗保留第二套文字按钮", () => {
    expect(captionPanelRaw).not.toContain("luna-caption__tool-btn");
    expect(captionPanelRaw).not.toContain("settingsShort");
    expect(captionPanelRaw).not.toContain("immersiveShort");
    expect(cssRaw).not.toContain(".luna-caption__tool-btn");
  });

  it("联网指示是表面级插槽：沉浸字幕与语音迷你形态都有挂载点，实现只有一份（ADR-111 ⑤⑴）", () => {
    // 字幕只有一枚工具药丸（ADR-137 H6），联网点挂在它上面；任何背景浓淡下它都不许被
    // display:none 藏掉——真机 2026-08-31 的破口正是「顶栏在沉浸态整体藏掉，音频照样离开设备」。
    // 会话中药丸只是变透明（悬停 / 聚焦浮现），挂载点始终在。
    expect(captionPanelRaw.match(/<HudHeader/g)?.length).toBe(1);
    expect(captionPanelRaw).toContain("uplink={executor}");
    expect(cssRaw).not.toMatch(/\.luna-caption[^{]*\.luna-caption__toolbar\s*\{[^}]*display:\s*none/s);
    expect(cssRaw).not.toMatch(/\.luna-caption\[data-immersive="true"\][^{]*\.luna-hud-header\s*\{[^}]*display:\s*none/s);
    // 语音同理：按住说话恰恰发生在胶囊档里——语音只有一张表面，联网点挂在它唯一的状态行上。
    expect(voiceHudRaw.match(/<HudHeader/g)?.length).toBe(1);
    expect(voiceHudRaw).toContain("uplink={uplink ? { ...uplink, active: sessionActive } : null}");
    // 三扇窗不再各抄一份 span；旧类名（借用录音 feature 的名字）零残留。
    for (const raw of [captionPanelRaw, recordingHudRaw, voiceHudRaw]) {
      expect(raw).not.toContain("luna-rec-hud__net-dot");
    }
    expect(cssRaw).toContain(".luna-hud-uplink-dot");
    expect(cssRaw).not.toContain(".luna-rec-hud__net-dot");
  });

  it("五扇 HUD 都有 Esc 逃生通道（ADR-111 ⑤⑵，批 B 起由 HudSurface 统一拥有）", () => {
    expect(hudSurfaceRaw).toContain('"Escape"');
    for (const raw of [
      popupTranslateRaw,
      voiceHudRaw,
      recordingHudRaw,
      captionPanelRaw,
      chatViewRaw,
    ]) {
      expect(raw).toContain("onDismiss=");
    }
  });

  it("翻译失败态塌缩为错误卡：不陈列空输出区与灰动作条（ADR-111 ⑤⑶）", () => {
    // 此前这条契约只有 `slot.status !== "idle"` 一个 grep 判据——failed 不是 idle，
    // 于是失败态照样陈列空 textarea 与灰动作条而门禁全绿。改为结构断言：失败分支
    // 早退，块内只有错误卡（含重试），没有输出区也没有动作条。
    const failureStart = popupResultRaw.indexOf("if (isFailure)");
    expect(failureStart).toBeGreaterThan(-1);
    const mainReturn = popupResultRaw.indexOf("\n  return (", failureStart);
    expect(mainReturn).toBeGreaterThan(failureStart);
    const failureBlock = popupResultRaw.slice(failureStart, mainReturn);
    // 失败卡自批 E2 起是 `FailureCard` 原语（唯一注册表 + 唯一深链表 + 重试）。
    expect(failureBlock).toContain("<FailureCard");
    expect(failureBlock).toContain("onRegenerate");
    expect(failureBlock).not.toContain("ResultActionBar");
    expect(failureBlock).not.toContain("luna-popup__output");
  });

  it("对话脚注只有一条提示，`@` 提示在 `@` 触发时才出现（ADR-111 ⑤⑷）", () => {
    expect(chatViewRaw).toContain('mentionQuery !== null ? tw("mention.hint") : t("input.hint")');
    // 提示占满余宽并省略，不再与知识库芯片/发送区互相挤压。
    expect(cssRaw).toMatch(/\.luna-chat__composer-hint\s*\{[^}]*flex:\s*1/s);
  });
});
