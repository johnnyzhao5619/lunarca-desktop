import { createContext } from "react";
import type { SettingsController } from "./use-settings-controller";

/** 设置宿主向嵌套面板提供同一份写入反馈，不创建第二套状态。 */
export const SettingsFieldContext = createContext<SettingsController | null>(null);
