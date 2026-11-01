import { useEffect, useRef } from "react";
import { commands, events, type SuggestionSurfaceDto } from "../../ipc/bindings";

export function shouldReportSuggestion(
  intersecting: boolean,
  documentVisible: boolean,
  alreadyReported: boolean,
  reporting = false,
) {
  return intersecting && documentVisible && !alreadyReported && !reporting;
}

/**
 * 只有建议卡确实进入可见视口、且文档本身可见时才上报曝光。预取、被运行态卡遮住、
 * 折叠区未展开与后台窗口都保持零事件；跨表面/重启去重由 Rust 的 HMAC 实体完成。
 */
/**
 * @param surface `null` = **这不是一枚主动提醒**（如「知识库 → 主题页」分区的恒显空态，
 *   TDD-07 §4.1c）：不上报曝光。曝光度量回答的是「这条主动冒出来的提醒被看见了吗」，
 *   把一个恒显的解释也报进去，等于让分母混进一类根本不需要被找回的表面。
 */
export function useSuggestionVisibility(
  suggestionToken: string | null,
  surface: SuggestionSurfaceDto | null,
) {
  const elementRef = useRef<HTMLDivElement>(null);
  const reportedRef = useRef<string | null>(null);
  const reportingRef = useRef<string | null>(null);

  useEffect(() => {
    const element = elementRef.current;
    if (!element || !suggestionToken || !surface || typeof IntersectionObserver === "undefined")
      return;
    let intersecting = false;
    const reportIfVisible = () => {
      if (
        shouldReportSuggestion(
          intersecting,
          document.visibilityState === "visible",
          reportedRef.current === suggestionToken,
          reportingRef.current === suggestionToken,
        )
      ) {
        reportingRef.current = suggestionToken;
        void commands
          .knowledgeSuggestionShown(suggestionToken, surface)
          .then((result) => {
            if (result.status === "ok" && result.data) reportedRef.current = suggestionToken;
          })
          .finally(() => {
            if (reportingRef.current === suggestionToken) reportingRef.current = null;
          });
      }
    };
    const observer = new IntersectionObserver(
      (entries) => {
        intersecting = entries.some((entry) => entry.target === element && entry.isIntersecting);
        reportIfVisible();
      },
      { threshold: 0.01 },
    );
    observer.observe(element);
    document.addEventListener("visibilitychange", reportIfVisible);
    const unlistenAvailability = events.knowledgeMetricsAvailability.listen(reportIfVisible);
    const unlistenSettings = events.settingsChanged.listen((event) => {
      if (event.payload.key === "privacy.local_knowledge_metrics") reportIfVisible();
    });
    return () => {
      document.removeEventListener("visibilitychange", reportIfVisible);
      void unlistenAvailability.then((dispose) => dispose());
      void unlistenSettings.then((dispose) => dispose());
      observer.disconnect();
    };
  }, [suggestionToken, surface]);

  return elementRef;
}
