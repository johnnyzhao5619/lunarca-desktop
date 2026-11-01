// 弹出编辑器独立窗口（PRD-07-036；Obsidian「移动到新窗口」对标）。
// 一窗一笔记：挂载时按窗口 label 取路径；窗口内点击 wiki-link 就地导航（替换当前笔记），
// 点击 #tag 跳主窗标签视图；关闭窗口时回主窗打开当前笔记（`notes://` 深链，单一导航语义）。

import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { VaultEditor } from "./VaultEditor";
import { noteStem } from "../../lib/note-path";
import { goTo } from "../../lib/navigate";

export function NoteWindow() {
  const { t } = useTranslation("editor");
  const [path, setPath] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [status, setStatus] = useState("");
  // 最新路径供关闭收尾读取（重命名/窗口内导航后仍回主窗正确的笔记）。
  const pathRef = useRef<string | null>(null);
  pathRef.current = path;

  useEffect(() => {
    let disposed = false;
    void commands.noteWindowPath().then((result) => {
      if (disposed) {
        return;
      }
      if (result.status === "ok" && result.data) {
        setPath(result.data);
      }
      setReady(true);
    });
    return () => {
      disposed = true;
    };
  }, []);

  // 窗口标题跟随当前笔记名（重命名/导航即时更新）。
  useEffect(() => {
    if (path) {
      void getCurrentWindow().setTitle(noteStem(path));
    }
  }, [path]);

  if (!ready) {
    return <div className="luna-note-window" data-loading="true" />;
  }

  if (!path) {
    return (
      <div className="luna-note-window luna-note-window--empty">
        <p>{t("window.missing")}</p>
      </div>
    );
  }

  return (
    <div className="luna-note-window">
      <VaultEditor
        path={path}
        onStatus={setStatus}
        onSavedPath={(savedPath) => setPath(savedPath)}
        onOpenPath={(openedPath) => setPath(openedPath)}
        // 独立窗口里点标签 = 回主窗看那个标签（与关闭回主窗同一语义）。走同一扇门
        // （`goTo` 在辅助窗里就是唤起主窗那条路），不再自己挑通道。
        onOpenTag={(tag) => goTo(`tags://${encodeURIComponent(tag)}`)}
        onWindowClose={async () => {
          const current = pathRef.current;
          if (current) {
            // 回主窗打开当前笔记（关闭独立窗口 = 回主窗，用户诉求）。
            await commands.openWorkspaceRoute(`notes://${current}`);
          }
        }}
      />
      <span className="luna-note-window__status" role="status">
        {status}
      </span>
    </div>
  );
}
