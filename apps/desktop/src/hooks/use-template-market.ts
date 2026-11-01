/**
 * 在线模板目录（纵切 7b，ADR-159 ③）：Rust `template_market` 的投影——「获取更多模板」里有哪些包、各是什么状态。
 *
 * 这里不判断任何事：这一块该不该出现（严格本地、未登录、服务端没发布模板目录时 Rust 回 `null`）、每一条是可获取 /
 * 已安装 / 可更新 / 需要更新 Lunarca、更新会多出哪些能力，都由 Rust 算好。命令零网络（读已验签目录的本地副本），
 * 模板目录换了一份（新条目、新版本、封禁）时随 `templates://changed` 重读。
 */

import { useCallback, useEffect, useState } from "react";

import { commands, events, type TemplateMarketDto } from "../ipc/bindings";

export interface TemplateMarket {
  /** `undefined` = 还没取回来；`null` = 这一块不出现。 */
  market: TemplateMarketDto | null | undefined;
  reload: () => void;
}

export function useTemplateMarket(): TemplateMarket {
  const [market, setMarket] = useState<TemplateMarketDto | null | undefined>(undefined);

  const reload = useCallback(() => {
    void commands.templateMarket().then(setMarket);
  }, []);

  useEffect(() => {
    reload();
    const unlisten = events.templatesChanged.listen(() => reload());
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [reload]);

  return { market, reload };
}
