// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { commandMock, resetBindingsDouble } from "../../ipc/bindings-test-double";
import { WebAccessControl } from "./WebAccessControl";

beforeEach(resetBindingsDouble);
afterEach(cleanup);

it("关闭前说明影响；确认后遇到失败，保留开关并显示错误", async () => {
  commandMock("accountWebAccess").mockResolvedValue({ status: "ok", data: true });
  const update = commandMock("accountSetWebAccess").mockResolvedValue({
    status: "error",
    error: { code: "web_access_unavailable" },
  });
  render(withI18n(<ConfirmHost><WebAccessControl /></ConfirmHost>));
  const toggle = await screen.findByRole("switch", { name: t("cloud:web_access.label") });
  expect(toggle.getAttribute("aria-checked")).toBe("true");
  fireEvent.click(toggle);

  const dialog = within(await screen.findByRole("alertdialog", { name: t("cloud:web_access.disable_title") }));
  expect(dialog.getByText(t("cloud:web_access.disable_description"))).toBeTruthy();
  expect(update).not.toHaveBeenCalled();
  fireEvent.click(dialog.getByRole("button", { name: t("cloud:web_access.disable_confirm") }));

  expect((await screen.findByRole("alert")).textContent).toBe(t("cloud:error.web_access_unavailable"));
  expect(update).toHaveBeenCalledWith(false);
  expect(toggle.getAttribute("aria-checked")).toBe("true");
});
