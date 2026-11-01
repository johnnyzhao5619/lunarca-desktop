import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { AppErrorBoundary } from "./components/AppErrorBoundary";
import { ConfirmHost } from "./components/ConfirmHost";
import { ContextMenuHost } from "./components/ContextMenuHost";
import { PromptHost } from "./components/PromptHost";
import { GlossaryQuickAdd } from "./features/settings/GlossaryQuickAdd";
import { readBootAppearance } from "./lib/boot";
import { applyInstalledThemeCss, applyTheme, resolveDensity, type ThemeMode } from "./theme/theme";
import "./i18n/i18n";
import "./styles/index.css";

// 首帧外观（PRD-14-066）：Rust 的注入脚本已在文档解析前把 data-* 挂到 <html> 上；这里
// 用同一份快照再走一遍前端的 `applyTheme`，保证 React 挂载前后是同一套映射。
const boot = readBootAppearance();
applyInstalledThemeCss(boot?.installed_theme_css ?? null);
if (boot) {
  applyTheme({
    mode: boot.theme as ThemeMode,
    family: boot.theme_family,
    reduceMotion: boot.reduce_motion,
    highContrast: boot.high_contrast,
    fontScale: boot.font_scale / 100,
    density: resolveDensity(boot.density),
  });
}

const container = document.getElementById("root");
if (!container) {
  throw new Error("找不到 #root 挂载点");
}

// 右键菜单宿主包住**所有窗口**（主窗 / HUD / 悬浮面板都走同一个 App 入口）：
// 原生「Reload / Inspect Element」不应在任何一扇窗里出现（PRD-07-059）。
createRoot(container).render(
  <StrictMode>
    <ContextMenuHost>
      {/* 破坏性确认宿主同层：删除动作从右键菜单、行内按钮、页头按钮三种入口发起，
          它们分布在整棵树上——确认必须是**同一张卡**，而不是每个入口各问各的。 */}
      <ConfirmHost>
        {/* 「起个名字」宿主同层：重命名是**对象**的能力，从文件树、四条清单、搜索结果、
            图谱节点都发得起——它们共用同一张输入卡，而不是各自长一套输入态。 */}
        <PromptHost>
          {/* 根级错误兜底：任何未捕获的渲染错误换成一张说明卡，不再整窗白屏。 */}
          <AppErrorBoundary>
            <App />
          </AppErrorBoundary>
          {/* 就地加词弹窗（PRD-07-061）：与右键菜单宿主同层——凡是能右键「添加到个人词典」
              的表面都在这层之内，各表面只派发事件，不必各自挂一个弹窗。事件未发生时不渲染。 */}
          <GlossaryQuickAdd />
        </PromptHost>
      </ConfirmHost>
    </ContextMenuHost>
  </StrictMode>,
);
