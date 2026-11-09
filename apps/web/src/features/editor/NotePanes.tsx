/**
 * 主区里的一篇笔记与它旁边那一份（PRD-21-031）：两边改了同一段时，冲突副本（本方的那份）与主文件
 * （别处的版本）并排打开，两份都能编辑。宽屏并排，窄屏单栏时变成可切换的两份（页签）。换了主笔记，旁边那一份
 * 收起。「查看对比」（PRD-21-030）是笔记里就地展开的逐行差异，不占旁边这一栏。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { FileSummary, WrittenFile } from "../../worker/protocol";
import type { WorkerClient } from "../../worker/worker-client";
import type { EditorMode } from "./editor-mode";
import type { NoteSession } from "./note-session";
import { NoteEditor } from "./NoteEditor";
import type { SessionRegistry } from "./session-registry";
import type { WebEditorHost } from "./web-editor-host";

/** 旁边那一份：冲突副本（或「另存我的这份」存下的那份）。 */
interface Companion {
  file: FileSummary;
}

export interface NotePanesProps {
  client: WorkerClient;
  host: WebEditorHost;
  registry: SessionRegistry;
  file: FileSummary;
  mode: EditorMode;
  onModeChange(mode: EditorMode): void;
  readOnly: string | null;
  onOpenPath(path: string): void;
  onNotText(fileId: string): void;
  onRename(file: FileSummary): void;
  onMove(file: FileSummary): void;
  onDelete(file: FileSummary, session: NoteSession | null): void;
}

export function NotePanes(props: NotePanesProps) {
  const { client, file } = props;
  const { t } = useTranslation("web");
  const [companion, setCompanion] = useState<Companion | null>(null);
  const [active, setActive] = useState<"main" | "companion">("main");

  useEffect(() => {
    setCompanion(null);
    setActive("main");
  }, [file.file_id]);

  async function openCopy(copy: WrittenFile) {
    const summary = await client.call("file_by_path", { path: copy.path }).catch(() => null);
    setCompanion({
      file: summary ?? { file_id: copy.file_id, path: copy.path, name: copy.path, size: 0, mtime_ms: null },
    });
  }

  const editorProps = {
    client,
    host: props.host,
    registry: props.registry,
    mode: props.mode,
    onModeChange: props.onModeChange,
    readOnly: props.readOnly,
    onOpenPath: props.onOpenPath,
    onNotText: props.onNotText,
    onRename: props.onRename,
    onMove: props.onMove,
  };

  return (
    <div className="luna-web-panes" data-companion={companion ? "copy" : undefined} data-active-pane={active}>
      {companion ? (
        <div className="luna-web-panes__tabs" role="tablist">
          <button type="button" role="tab" className="luna-web-tabs__tab" aria-selected={active === "main"} onClick={() => setActive("main")}>
            {t("note.panes.main")}
          </button>
          <button type="button" role="tab" className="luna-web-tabs__tab" aria-selected={active === "companion"} onClick={() => setActive("companion")}>
            {t("note.panes.copy")}
          </button>
        </div>
      ) : null}
      <div className="luna-web-panes__pane" data-pane="main">
        <NoteEditor
          {...editorProps}
          file={file}
          onConflictCopy={(copy) => void openCopy(copy)}
          onDelete={props.onDelete}
        />
      </div>
      {companion ? (
        <div className="luna-web-panes__pane" data-pane="companion" data-testid="companion-copy">
          <NoteEditor
            {...editorProps}
            file={companion.file}
            onConflictCopy={(copy) => void openCopy(copy)}
            onDelete={(target, session) => {
              setCompanion(null);
              props.onDelete(target, session);
            }}
          />
          <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => setCompanion(null)}>
            {t("note.panes.close")}
          </button>
        </div>
      ) : null}
    </div>
  );
}
