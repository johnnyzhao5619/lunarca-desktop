// 设置 / 模型数据层（TDD-07）。仅做生成 IPC 命令的薄封装 + 无乐观更新的刷新（写后重读，
// 事实源恒在 Rust，ADR-003）。无业务校验——校验全在 Rust set_setting/registry。

import { useCallback, useEffect, useState } from "react";
import {
  commands,
  events,
  type ComputeServiceDto,
  type CoremlPhase,
  type ModelAssignmentDto,
  type ModelEntryDto,
  type ModelDeletePlanDto,
  type ModelDownloadProgress,
  type ModelInstallPlanDto,
  type RemoteComputeDto,
  type SettingSpecDto,
  type SettingValueDto,
  type StorageInfoDto,
} from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

export const SETTINGS_CHANGED_EVENT = "luna:settings-changed";

function notifySettingsChanged(): void {
  window.dispatchEvent(new Event(SETTINGS_CHANGED_EVENT));
}

export function useSettings() {
  const [specs, setSpecs] = useState<SettingSpecDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    const result = await commands.listSettings();
    if (result.status === "ok") {
      setSpecs(result.data);
      setError(null);
    } else {
      setError(vaultErrorText(result.error));
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const setValue = useCallback(
    async (key: string, value: SettingValueDto) => {
      const result = await commands.setSetting(key, value);
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        return;
      }
      await refresh();
      notifySettingsChanged();
    },
    [refresh],
  );

  const reset = useCallback(
    async (key: string) => {
      const result = await commands.resetSetting(key);
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        return;
      }
      await refresh();
      notifySettingsChanged();
    },
    [refresh],
  );

  return { specs, loading, error, setValue, reset, refresh };
}

/**
 * 单个键的注册表声明。
 *
 * 给**面板承载的键**用（ADR-122 ⑤）：面板要渲染的是注册表声明的控件形态，而不是自己
 * 手写一个——手写的那个迟早与声明分叉，而分叉时没有任何东西会变红。转写同意就是这么
 * 分叉的：注册表写着 `Toggle`，承载它的算力卡手写了一个原生 `<input type="checkbox">`，
 * 于是全应用唯一一个原生勾选框正好落在那条隐私同意上。
 *
 * 跟着 `settings://changed` 重读：这个键在别处（搜索深链、恢复默认）也改得动。
 */
export function useSettingSpec(key: string): SettingSpecDto | undefined {
  const [spec, setSpec] = useState<SettingSpecDto | undefined>(undefined);
  useEffect(() => {
    const load = () => {
      void commands.listSettings().then((result) => {
        if (result.status === "ok") setSpec(result.data.find((item) => item.key === key));
      });
    };
    load();
    const unlisten = events.settingsChanged.listen(load);
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [key]);
  return spec;
}

export function useModels() {
  const [models, setModels] = useState<ModelEntryDto[]>([]);
  const [assignments, setAssignments] = useState<ModelAssignmentDto[]>([]);
  const [storage, setStorage] = useState<StorageInfoDto | null>(null);
  const [services, setServices] = useState<ComputeServiceDto[]>([]);
  // 计算服务端点是否接管了 LLM——由 Rust 判定并下发（前端不读设置项自行推断，ADR-003）。
  const [remoteCompute, setRemoteCompute] = useState<RemoteComputeDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [progressById, setProgressById] = useState<Record<string, ModelDownloadProgress>>({});
  /** CoreML 编码器的安装阶段（`model://coreml-phase`），按模型 id；终态到达即重读清单。 */
  const [coremlPhaseById, setCoremlPhaseById] = useState<Record<string, CoremlPhase>>({});

  const refresh = useCallback(async () => {
    const snapshot = await commands.modelManagementSnapshot();
    if (snapshot.status === "ok") {
      setModels(snapshot.data.models);
      setAssignments(snapshot.data.assignments);
      setStorage(snapshot.data.storage);
      setServices(snapshot.data.services);
      setRemoteCompute(snapshot.data.remote_compute);
      setError(null);
    } else {
      setError(vaultErrorText(snapshot.error));
      setModels([]);
      setAssignments([]);
      setStorage(null);
      setServices([]);
      setRemoteCompute(null);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const handleSettingsChanged = () => {
      void refresh();
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, handleSettingsChanged);
    return () => {
      window.removeEventListener(SETTINGS_CHANGED_EVENT, handleSettingsChanged);
    };
  }, [refresh]);

  useEffect(() => {
    const unlisten = events.modelCoremlPhase.listen((event) => {
      setCoremlPhaseById((current) => ({ ...current, [event.payload.model_id]: event.payload }));
      if (
        event.payload.phase === "ready" ||
        event.payload.phase === "failed" ||
        event.payload.phase === "removed"
      ) {
        void refresh();
      }
    });
    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, [refresh]);

  useEffect(() => {
    const unlisten = events.modelDownloadProgress.listen((event) => {
      if (event.payload.paused) {
        // 暂停（PRD-14-020）：进度条收起、清单重读（断点字节数随条目回来），不算完成。
        setProgressById((current) => {
          const next = { ...current };
          delete next[event.payload.model_id];
          return next;
        });
        void refresh();
        return;
      }
      setProgressById((current) => ({
        ...current,
        [event.payload.model_id]: event.payload,
      }));
      if (event.payload.done) {
        void refresh();
      }
    });
    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, [refresh]);

  const planDelete = useCallback(async (id: string): Promise<ModelDeletePlanDto | null> => {
    const result = await commands.planDeleteModel(id);
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      setMessage(null);
      return null;
    }
    setError(null);
    return result.data;
  }, []);

  const startDelete = useCallback(
    async (planId: string): Promise<boolean> => {
      const result = await commands.startDeleteModel(planId);
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        setMessage(null);
        return false;
      }
      setError(null);
      setMessage(null);
      await refresh();
      return true;
    },
    [refresh],
  );

  const planInstall = useCallback(async (id: string): Promise<ModelInstallPlanDto | null> => {
    const result = await commands.planInstallModel(id);
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      setMessage(null);
      return null;
    }
    setError(null);
    return result.data;
  }, []);

  const startInstall = useCallback(async (planId: string): Promise<string | null> => {
    // 开始（含从断点续）即清掉上一条通知：「已暂停下载」不该在进度条已经在走的时候还挂着
    // （真机 2026-09-06 截图坐实）。
    setMessage(null);
    const result = await commands.startInstallModel(planId);
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      setMessage(null);
      return null;
    }
    setError(null);
    setMessage(result.data);
    await refresh();
    return result.data;
  }, [refresh]);

  /** 启用 CoreML 加速（下载编码器 → 解压 → 子进程探针编译；ADR-115）。 */
  const installCoremlEncoder = useCallback(async (id: string): Promise<boolean> => {
    setMessage(null);
    const result = await commands.installCoremlEncoder(id);
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      return false;
    }
    setError(null);
    setMessage(result.data);
    return true;
  }, []);

  /** 停用 CoreML 加速（移除编码器，权重不动）。 */
  const removeCoremlEncoder = useCallback(
    async (id: string): Promise<boolean> => {
      const result = await commands.removeCoremlEncoder(id);
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        return false;
      }
      setError(null);
      setMessage(null);
      await refresh();
      return true;
    },
    [refresh],
  );

  /** 暂停一次进行中的下载 / 拉取（PRD-14-020）；进度条的收起由 `paused` 帧驱动。 */
  const pauseInstall = useCallback(async (id: string): Promise<boolean> => {
    const result = await commands.pauseInstallModel(id);
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      return false;
    }
    return true;
  }, []);

  // 首启/HUD 的卡片本身已经展示模型与下载动作；仍先消费 Rust 计划并仅在可执行时开始。
  const install = useCallback(async (id: string): Promise<string | null> => {
    const plan = await planInstall(id);
    if (!plan) return null;
    if (!plan.can_start) {
      setError(plan.blocking_reason ?? "MODEL_INSTALL_BLOCKED");
      return null;
    }
    return startInstall(plan.plan_id);
  }, [planInstall, startInstall]);

  const assign = useCallback(
    async (slot: string, targetId: string): Promise<boolean> => {
      const result = await commands.assignModel(slot, targetId);
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        setMessage(null);
        return false;
      }
      setError(null);
      setMessage(null);
      await refresh();
      notifySettingsChanged();
      return true;
    },
    [refresh],
  );

  const clearAssignment = useCallback(
    async (slot: string): Promise<boolean> => {
      const result = await commands.clearModelAssignment(slot);
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        setMessage(null);
        return false;
      }
      setError(null);
      setMessage(null);
      await refresh();
      notifySettingsChanged();
      return true;
    },
    [refresh],
  );

  return {
    models,
    assignments,
    storage,
    services,
    remoteCompute,
    error,
    message,
    progressById,
    coremlPhaseById,
    installCoremlEncoder,
    removeCoremlEncoder,
    planDelete,
    startDelete,
    planInstall,
    startInstall,
    pauseInstall,
    install,
    assign,
    clearAssignment,
    refresh,
  };
}

/** 模型面板族共用的控制器句柄——每处各写一遍 `ReturnType<typeof useModels>` 是同一件事的第二份定义。 */
export type ModelsController = ReturnType<typeof useModels>;
