import { Component, type ReactNode } from "react";
import { i18n } from "../i18n/i18n";

interface State {
  error: Error | null;
}

/**
 * 每扇窗的根级错误兜底：渲染或 effect 里任何一处未捕获的错误，此前会让 React 卸掉整棵树、
 * 窗口直接白屏且没有任何说明（真机 2026-10-04：源码模式切回实时预览）。现在换成一张说明卡——
 * 发生了什么、已保存的内容是否受影响、怎么继续——外加可展开的技术细节，便于报告问题。
 *
 * **不往日志里写错误信息**：错误消息偶尔会带上正文片段，日志纪律不允许（AGENTS §7）；细节只在
 * 用户自己的屏幕上，复制与否由用户决定。类组件是 React 捕获渲染错误的唯一写法，故文案直接走
 * `i18n.t`（不在 hooks 里）。
 */
export class AppErrorBoundary extends Component<{ children: ReactNode }, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  override render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const t = (key: string) => i18n.t(key, { ns: "common" });
    // 技术细节是错误自身的事实（名字、消息、调用栈），不是界面文案。
    const detail = [`${error.name}: ${error.message}`, error.stack ?? ""].join("\n");
    return (
      <main className="luna-crash" role="alert">
        <h1 className="luna-crash__title">{t("crash.title")}</h1>
        <p className="luna-crash__body">{t("crash.body")}</p>
        <button type="button" className="luna-btn luna-btn--primary" onClick={() => window.location.reload()}>
          {t("crash.reload")}
        </button>
        <details className="luna-crash__details">
          <summary>{t("crash.details")}</summary>
          <pre className="luna-crash__stack">{detail}</pre>
        </details>
      </main>
    );
  }
}
