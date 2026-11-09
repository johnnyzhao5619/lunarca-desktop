/**
 * 设置中心控制器（`useChatController` 同范式）：主侧栏的页面导航与主区的设置面板
 * **共享同一实例**，否则搜索词、当前页、写入后的重读会各持一份而分叉。
 *
 * 数据层仍是薄封装（ADR-003）：事实源恒在 Rust——写入命令返回 Rust 归一化后的权威值，
 * 完整重读由 `settings://changed` 广播驱动（也让别处改的设置即时反映）；这里只持有 UI 态
 * （当前页 / 搜索词 / 最近保存反馈 / 逐项错误）。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  events,
  type SettingSpecDto,
  type SettingValueDto,
  type ResetScope,
} from "../../ipc/bindings";
import { SETTINGS_CHANGED_EVENT } from "./use-settings";
import {
  modifiedCountByPage,
  orderedPages,
  visibleSpecs,
} from "./settings-filter";
import { type SettingsPageId } from "./settings-information-architecture";
import { vaultErrorText } from "../../lib/vault-error";

/** 「已保存」微反馈的驻留时长（ms）：够看清、不拖泥带水。 */
const SAVED_FLASH_MS = 1600;

export interface SettingsController {
  /** 用户可见的设置项（已剔除内部状态与未启用能力）。 */
  specs: SettingSpecDto[];
  /** 完整注册表快照。只用于搜索高级命中与切换显示投影，不是第二份配置。 */
  allSpecs: SettingSpecDto[];
  advancedMode: boolean;
  setAdvancedMode: (enabled: boolean) => Promise<boolean>;
  /** 有内容的页（按信息架构顺序；标准投影下只剩高级内容的页不在其中）。 */
  pages: SettingsPageId[];
  /** 当前页；`""` = 规格尚未载入、还没落到任何一页。 */
  activePage: SettingsPageId | "";
  /** 切到某页。 */
  selectPage: (page: SettingsPageId) => void;
  query: string;
  setQuery: (query: string) => void;
  /** 各页已改动项数（页头「恢复…」按钮及其确认文案、侧栏右键菜单的可用性）。 */
  modifiedByPage: Partial<Record<SettingsPageId, number>>;
  /** 逐项写入错误（key → Rust 错误串，含稳定 code）。 */
  /**
   * 写不进去的那些行：错误串 + 这次试的是什么值。带上 `attempted` 是因为写失败时行上显示
   * 的仍是旧值，只说"冲突了"用户找不到那句话在讲谁。
   */
  errors: Record<string, { message: string; attempted: SettingValueDto }>;
  /** 清掉某一行的错误（用户开始重录时调：他正要换掉那个值，旧的判语不该留在屏幕上）。 */
  clearError: (key: string) => void;
  /** 各项独立保存反馈与显式恢复次数（恢复后丢弃该行的未提交草稿）。 */
  savedKeys: Record<string, boolean>;
  pendingKeys: Record<string, boolean>;
  resetEpochs: Record<string, number>;
  modifiedOnly: boolean;
  setModifiedOnly: (enabled: boolean) => void;
  /** 快捷键运行诊断（key → code，PRD-09-003 ②③）。 */
  diagnostics: Record<string, string>;
  loading: boolean;
  /** 取数失败（整页级）。 */
  error: string | null;
  setValue: (key: string, value: SettingValueDto) => Promise<SettingValueDto | null>;
  reset: (key: string) => Promise<void>;
  /** 恢复一整页（范围默认随当前模式：标准 → 常用项，高级 → 全部项）；返回恢复条数。 */
  resetPage: (page: SettingsPageId, scope: ResetScope, fingerprint: string) => Promise<number | null>;
  refresh: () => Promise<void>;
}

export function useSettingsController(enabled: boolean): SettingsController {
  const [rawSpecs, setRawSpecs] = useState<SettingSpecDto[]>([]);
  const [diagnostics, setDiagnostics] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  // 注册表至少成功读回过一次。在此之前 `pages` 只剩面板型页（它们不靠设置行也照常呈现），
  // 拿它挑「首页」会落到「AI 服务」而不是「通用」（真机 2026-11-03）。
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [errors, setErrors] = useState<
    Record<string, { message: string; attempted: SettingValueDto }>
  >({});
  const [savedKeys, setSavedKeys] = useState<Record<string, boolean>>({});
  const [pendingKeys, setPendingKeys] = useState<Record<string, boolean>>({});
  const [resetEpochs, setResetEpochs] = useState<Record<string, number>>({});
  const pending = useRef(new Set<string>());
  const resettingPage = useRef(false);
  const [modifiedOnly, setModifiedOnly] = useState(false);
  const [query, setQueryState] = useState("");
  const [activePage, setActivePage] = useState<SettingsPageId | "">("");
  const savedTimers = useRef(new Map<string, number>());
  const revision = useRef(0);
  const refreshSequence = useRef(0);

  const refresh = useCallback(async () => {
    const sequence = ++refreshSequence.current;
    const atRevision = revision.current;
    const [list, diag] = await Promise.all([
      commands.listSettings(),
      commands.shortcutDiagnostics(),
    ]);
    if (sequence !== refreshSequence.current || atRevision !== revision.current) return;
    if (list.status === "ok") {
      setRawSpecs(list.data);
      setLoaded(true);
      setError((current) => current === "SETTING_RESET_STALE" ? current : null);
    } else {
      setError(vaultErrorText(list.error));
    }
    if (diag.status === "ok") {
      setDiagnostics(Object.fromEntries(diag.data.map((item) => [item.key, item.code])));
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
  }, [enabled, refresh]);

  // 广播覆盖其他入口的写入；本页写入结束后再读，避免广播早于回执到达而被版本保护丢弃。
  useEffect(() => {
    if (!enabled) return;
    const unlisten = events.settingsChanged.listen(() => {
      void refresh();
    });
    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, [enabled, refresh]);

  // 快捷键诊断随**系统状态**变（辅助功能权限、他应用抢占），不随设置变：用户去系统设置
  // 授完权切回来，焦点回归即重拉一次，那一行的提示当场消失（与引导页权限步同一手法）。
  useEffect(() => {
    if (!enabled) return;
    const onFocus = () => {
      void commands.shortcutDiagnostics().then((diag) => {
        if (diag.status === "ok") {
          setDiagnostics(Object.fromEntries(diag.data.map((item) => [item.key, item.code])));
        }
      });
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.removeEventListener("focus", onFocus);
    };
  }, [enabled]);

  useEffect(() => {
    return () => {
      for (const timer of savedTimers.current.values()) window.clearTimeout(timer);
    };
  }, []);

  const specs = useMemo(
    () =>
      visibleSpecs(rawSpecs, {
        advancedMode:
          rawSpecs.find((spec) => spec.key === "settings.advanced_mode")?.current_value.value === true,
      }),
    [rawSpecs],
  );
  const advancedMode = rawSpecs.find((spec) => spec.key === "settings.advanced_mode")?.current_value.value === true;
  const pages = useMemo(() => orderedPages(specs), [specs]);
  const modifiedByPage = useMemo(() => modifiedCountByPage(specs), [specs]);

  // 首次载入落到第一页；当前页消失（如关掉高级模式后「系统」整页退出标准投影）时回落，
  // 绝不停在空白页。**等注册表读回来再挑**：控制器随工作区一起挂载，早于第一次取数，
  // 那时的页面清单不是真的清单。
  useEffect(() => {
    if (!loaded || pages.length === 0) return;
    if (!pages.includes(activePage as SettingsPageId)) {
      setActivePage(pages[0]);
    }
  }, [loaded, pages, activePage]);

  const selectPage = useCallback((page: SettingsPageId) => {
    setActivePage(page);
  }, []);

  const flashSaved = useCallback((key: string) => {
    setSavedKeys((current) => ({ ...current, [key]: true }));
    const previous = savedTimers.current.get(key);
    if (previous !== undefined) window.clearTimeout(previous);
    savedTimers.current.set(key, window.setTimeout(() => {
      setSavedKeys((current) => { const next = { ...current }; delete next[key]; return next; });
      savedTimers.current.delete(key);
    }, SAVED_FLASH_MS));
  }, []);

  const clearError = useCallback((key: string) => {
    setErrors((current) => {
      if (!(key in current)) {
        return current;
      }
      const next = { ...current };
      delete next[key];
      return next;
    });
  }, []);

  const setValue = useCallback(
    async (key: string, value: SettingValueDto): Promise<SettingValueDto | null> => {
      if (resettingPage.current || pending.current.has(key)) return null;
      pending.current.add(key);
      revision.current++;
      setPendingKeys((current) => ({ ...current, [key]: true }));
      setSavedKeys((current) => { const next = { ...current }; delete next[key]; return next; });
      try {
      const result = await commands.setSetting(key, value);
      if (result.status === "error") {
        // 校验失败是**行级**信息（哪一行写不进去），不该顶到页头当全局错误。
        setErrors((current) => ({
          ...current,
          [key]: { message: result.error, attempted: value },
        }));
        return null;
      }
      // 命令返回的值已经过 Rust 的 normalize + validate。先就地采用它，避免把原始输入
      // 当事实；changed 广播随后仍会重读整份注册表，覆盖副作用导致的其他项变化。
      setRawSpecs((current) =>
        current.map((spec) =>
          spec.key === key ? { ...spec, current_value: result.data } : spec,
        ),
      );
      clearError(key);
      flashSaved(key);
      window.dispatchEvent(new Event(SETTINGS_CHANGED_EVENT));
      return result.data;
      } catch (cause) {
        setErrors((current) => ({ ...current, [key]: { message: String(cause), attempted: value } }));
        return null;
      } finally {
        pending.current.delete(key);
        revision.current++;
        setPendingKeys((current) => { const next = { ...current }; delete next[key]; return next; });
        void refresh();
      }
    },
    [clearError, flashSaved, refresh],
  );

  const reset = useCallback(
    async (key: string) => {
      if (resettingPage.current || pending.current.has(key)) return;
      pending.current.add(key);
      revision.current++;
      setPendingKeys((current) => ({ ...current, [key]: true }));
      try {
      const result = await commands.resetSetting(key);
      if (result.status === "error") {
        // 恢复默认没有"这次试的值"——那是注册表里的默认值，界面上说得清。
        setErrors((current) => ({
          ...current,
          [key]: { message: result.error, attempted: { kind: "str", value: "" } },
        }));
        return;
      }
      clearError(key);
      flashSaved(key);
      setResetEpochs((current) => ({ ...current, [key]: (current[key] ?? 0) + 1 }));
      window.dispatchEvent(new Event(SETTINGS_CHANGED_EVENT));
      } catch (cause) {
        setErrors((current) => ({ ...current, [key]: {
          message: String(cause), attempted: { kind: "str", value: "" },
        } }));
      } finally {
        pending.current.delete(key);
        revision.current++;
        setPendingKeys((current) => { const next = { ...current }; delete next[key]; return next; });
        void refresh();
      }
    },
    [clearError, flashSaved, refresh],
  );

  const resetPage = useCallback(
    async (
      page: SettingsPageId,
      scope: ResetScope,
      fingerprint: string,
    ): Promise<number | null> => {
      if (resettingPage.current || pending.current.size > 0) return null;
      resettingPage.current = true;
      setError(null);
      setPendingKeys(Object.fromEntries(rawSpecs.map((spec) => [spec.key, true])));
      revision.current++;
      try {
      const result = await commands.resetSettingGroup(page, scope, fingerprint);
      if (result.status === "error") {
        setError(result.error.startsWith("SETTING_RESET_STALE") ? "SETTING_RESET_STALE" : vaultErrorText(result.error));
        return null;
      }
      setErrors({});
      setResetEpochs((current) => {
        const next = { ...current };
        for (const spec of rawSpecs.filter((item) => item.group === page)) {
          next[spec.key] = (next[spec.key] ?? 0) + 1;
        }
        return next;
      });
      window.dispatchEvent(new Event(SETTINGS_CHANGED_EVENT));
      return result.data;
      } catch (cause) {
        setError(vaultErrorText(String(cause)));
        return null;
      } finally {
        resettingPage.current = false;
        setPendingKeys({});
        revision.current++;
        await refresh();
      }
    },
    [rawSpecs, refresh],
  );

  const setAdvancedMode = useCallback(
    async (enabled: boolean) =>
      (await setValue("settings.advanced_mode", { kind: "bool", value: enabled })) !== null,
    [setValue],
  );

  const setQuery = useCallback((nextQuery: string) => {
    setQueryState(nextQuery);
  }, []);

  return {
    specs,
    allSpecs: rawSpecs,
    advancedMode,
    setAdvancedMode,
    pages,
    activePage,
    selectPage,
    query,
    setQuery,
    modifiedByPage,
    errors,
    clearError,
    savedKeys,
    pendingKeys,
    resetEpochs,
    modifiedOnly,
    setModifiedOnly,
    diagnostics,
    loading,
    error,
    setValue,
    reset,
    resetPage,
    refresh,
  };
}
