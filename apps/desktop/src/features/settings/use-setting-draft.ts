import { useEffect, useRef, useState } from "react";

/** 刷新不覆盖未提交输入，迟到回执不覆盖提交后的新编辑。 */
export function useSettingDraft<T extends string | number>(current: T) {
  const [draft, setValue] = useState(current);
  const dirty = useRef(false);
  const version = useRef(0);
  useEffect(() => {
    if (!dirty.current) setValue(current);
  }, [current]);
  return {
    draft,
    version,
    setDraft(value: T) {
      dirty.current = true;
      version.current++;
      setValue(value);
    },
    adopt(value: T, submittedVersion: number) {
      if (submittedVersion !== version.current) return;
      dirty.current = false;
      setValue(value);
    },
  };
}
