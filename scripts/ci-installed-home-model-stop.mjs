import { assertJourneyEqual } from "./ci-installed-home-failure.mjs";

export async function stopInstalledModelRun({ assistant, startTurn, resourceAction }) {
  // Bind the warm engine from the preceding reply before starting this run.
  // Text may arrive with a terminal; Runtime acceptance is the Stop trigger.
  await resourceAction("engine_running");
  const owner = await startTurn();
  assertJourneyEqual("stop_create_terminal", Boolean(owner.run.terminal), false);
  const cancelled = assistant.page().waitForResponse(response => response.url().endsWith("/api/provider/model/runs_cancel") &&
    response.request().frame() === assistant && response.request().postDataJSON()?.run_id === owner.run.run_id, { timeout: 30000 });
  cancelled.catch(() => {});
  await assistant.locator('#agent-composer-send[data-mode="stop"]').click();
  assertJourneyEqual("stop_cancel_http_ok", (await cancelled).ok(), true);
  assertJourneyEqual("stop_terminal_status", (await owner.terminal).status, "cancelled");
  const closed = await resourceAction("engine_stopped");
  assertJourneyEqual("stop_whole_engine_exit", closed.whole_engine_exit, true);
  assertJourneyEqual("stop_runtime_unchanged", closed.runtime_unchanged, true);
  await assistant.locator('#agent-composer-send[data-mode="send"]').waitFor({ state: "visible", timeout: 10000 });
  return owner;
}
