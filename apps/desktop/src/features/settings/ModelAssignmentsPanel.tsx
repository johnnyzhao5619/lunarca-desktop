import { useTranslation } from "react-i18next";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { SlotRow } from "./ModelManagementPanel";
import { useModels } from "./use-settings";

/** AI 服务页的唯一模型分配入口，候选资格与继承规则由 Rust 下发。 */
export function ModelAssignmentsPanel() {
  const { t } = useTranslation("settings");
  const { assignments, assign, clearAssignment, error } = useModels();
  return (
    <section id="settings-section-assignments" tabIndex={-1} className="luna-settings-panel luna-settings-card">
      <header className="luna-settings-panel__header"><h2>{t("models.view.assignments")}
        <HelpDetails summary={t("models.view.assignments")}><p>{t("models.assignment_intro")}</p></HelpDetails>
      </h2></header>
      {error ? <p className="luna-error-banner" role="alert">{error}</p> : null}
      <ul className="luna-settings-models__list">
        {assignments.map((assignment) => <SlotRow key={assignment.slot} assignment={assignment} assign={assign} clearAssignment={clearAssignment} />)}
      </ul>
    </section>
  );
}
