import type { SettingSpecDto } from "../../ipc/bindings";
import type { SetValue } from "./SettingControls";
import { SettingField } from "./SettingField";

/** 面板和列表共用设置行、写入反馈与动作槽。 */
export function PanelSettingRow({
  spec,
  setValue,
  desc,
  disabled,
}: {
  spec: SettingSpecDto | undefined;
  setValue: SetValue;
  desc?: string;
  disabled?: boolean;
}) {
  if (!spec) return null;
  return <SettingField spec={spec} setValue={setValue} desc={desc} disabled={disabled} panel />;
}
