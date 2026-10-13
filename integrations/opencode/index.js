// ctx for opencode: each bash call runs in the session's ctx tab (`ctx hook opencode` decides how).
import { execFileSync } from "node:child_process";

const CTX = "ctx";

export default {
  id: "ctx",
  server: async ({ directory }) => ({
    "tool.execute.before": async (input, output) => {
      const args = output.args;
      if (input.tool !== "bash" || typeof args?.command !== "string") return;
      try {
        const reply = execFileSync(CTX, ["hook", "opencode"], {
          input: JSON.stringify({
            session_id: input.sessionID,
            cwd: args.workdir ?? directory,
            command: args.command,
            background: args.background === true,
            timeout_ms: typeof args.timeout === "number" ? args.timeout : undefined,
          }),
          encoding: "utf8",
          timeout: 10_000,
        });
        const { command } = JSON.parse(reply);
        if (typeof command !== "string") return;
        args.command = command;
        if (args.background === true) args.background = false;
      } catch {
        // ctx isn't reachable: the command runs as it would have.
      }
    },
  }),
};
