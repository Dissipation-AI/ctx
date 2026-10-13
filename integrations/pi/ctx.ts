// ctx for pi: each bash call runs in the session's ctx tab (`ctx hook pi` decides how).
import { execFileSync } from "node:child_process";

const CTX = "ctx";

export default function (pi: any) {
  pi.on("tool_call", (event: any, ctx: any) => {
    if (event.toolName !== "bash" || typeof event.input?.command !== "string") return;
    try {
      const reply = execFileSync(CTX, ["hook", "pi"], {
        input: JSON.stringify({
          session_id: ctx.sessionManager?.getSessionId?.() ?? "",
          cwd: ctx.cwd,
          command: event.input.command,
          timeout_ms: typeof event.input.timeout === "number" ? event.input.timeout * 1000 : undefined,
        }),
        encoding: "utf8",
        timeout: 10_000,
      });
      const { command } = JSON.parse(reply);
      if (typeof command === "string") event.input.command = command;
    } catch {
      // ctx isn't reachable: the command runs as it would have.
    }
  });
}
