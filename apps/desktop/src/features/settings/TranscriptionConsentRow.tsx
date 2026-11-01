/**
 * 「录音转写也交给服务端」的同意项（ADR-038 第 4 条）。
 *
 * 设置页的算力卡与首启引导都要问这一句，故呈现与判断共用一份；写入路径各留各的：
 * 两侧的错误面不同（行级 + 广播 vs 就地提示 + 重读落点），强行合并会让其中一个说不出话。
 *
 * 行外壳与控件都走 `PanelSettingRow`（ADR-122 第 5 条 / ADR-125 第 18 条）：同一张卡上
 * 另一个可改项（优先落点）用的就是它。
 */

import { useTranslation } from "react-i18next";
import type {
  ComputeStatusDto,
  SettingSpecDto,
  TranscriptionServerBlocker,
} from "../../ipc/bindings";
import { PanelSettingRow } from "./PanelSettingRow";

/** 转写同意的设置键，唯一定义点。 */
export const TRANSCRIPTION_CONSENT_KEY = "provider.remote_asr_enabled";

/**
 * 勾不动时换成说明原因的那一句。穷尽 Record：Rust 新增一种原因时 tsc 立刻红，
 * 而不是让开关灰着却什么都不解释。`no_server` 那一档这一行根本不渲染，给一句以防万一。
 */
const BLOCKER_COPY: Record<TranscriptionServerBlocker, string> = {
  no_server: "compute_card.reason.device_policy",
  local_only_strategy: "compute_card.transcription_local_only",
  server_not_configured: "compute_card.reason.server_not_configured",
};

/**
 * 转写此刻是不是在服务端跑——从 Rust 下发的 `routes` 里取而不是另存一个布尔：
 * 位置与理由必须来自同一条记录，否则开关与它旁边那句「为什么」会各说各的。
 */
export function transcriptionOnServer(status: ComputeStatusDto): boolean {
  return status.routes.find((route) => route.capability === "transcription")?.location === "server";
}

export function TranscriptionConsentRow({
  status,
  spec,
  onChange,
}: {
  status: ComputeStatusDto;
  /** 注册表声明（ADR-122 第 5 条）。由宿主传入：设置页的控制器本就握着它，引导侧自取。 */
  spec: SettingSpecDto | undefined;
  onChange: (value: boolean) => void;
}) {
  const { t } = useTranslation("settings");
  const blocker = status.transcription_server_blocker;
  return (
    <div className="luna-compute-card__consent">
      {/* 不可用时照常显示、但换成说明原因的那一句：隐藏它等于让用户以为这个能力不存在，
          摆一个勾不动又不解释的开关等于告诉他「软件坏了」。可用时说明取注册表那一份——
          搜索命中显示的也是它。 */}
      <PanelSettingRow
        spec={spec}
        disabled={blocker !== null}
        desc={blocker === null ? undefined : t(BLOCKER_COPY[blocker])}
        setValue={(_key, value) => {
          onChange(value.kind === "bool" && value.value);
          return Promise.resolve(null);
        }}
      />
    </div>
  );
}
