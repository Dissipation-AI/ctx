// ctx for opencode: each shell call runs in the session's ctx tab (`ctx hook opencode` decides how).
// One export for both plugin APIs: opencode 2 reads `id` and `setup`, opencode 1 calls `server`.
import { execFileSync } from "node:child_process";

const CTX = "ctx";

// The rewritten command for a shell call's arguments, or undefined to leave it alone.
function rewrite(sessionID, cwd, args) {
  if (typeof args?.command !== "string") return undefined;
  try {
    const reply = execFileSync(CTX, ["hook", "opencode"], {
      input: JSON.stringify({
        session_id: sessionID,
        cwd: args.workdir ?? cwd,
        command: args.command,
        background: args.background === true,
        timeout_ms: typeof args.timeout === "number" ? args.timeout : undefined,
      }),
      encoding: "utf8",
      timeout: 10_000,
    });
    const { command } = JSON.parse(reply);
    return typeof command === "string" ? command : undefined;
  } catch {
    // ctx isn't reachable: the command runs as it would have.
    return undefined;
  }
}

// The folder a session works in (opencode 2), or opencode's own when it can't say.
async function sessionFolder(ctx, sessionID) {
  for (const ask of [() => ctx.session.get({ sessionID }), () => ctx.session.get(sessionID)]) {
    try {
      const s = await ask();
      const dir = s?.location?.directory ?? s?.data?.location?.directory ?? s?.directory;
      if (typeof dir === "string") return dir;
    } catch {
      // try the other form
    }
  }
  return process.cwd();
}

export default {
  id: "ctx",
  async setup(ctx) {
    const hook = await ctx.tool.hook("execute.before", async (event) => {
      if (event.tool !== "shell" && event.tool !== "bash") return;
      const args = event.input;
      if (typeof args?.command !== "string") return;
      const cwd = args.workdir ?? (await sessionFolder(ctx, event.sessionID));
      const command = rewrite(event.sessionID, cwd, args);
      if (command === undefined) return;
      event.input = { ...args, command, background: false };
    });
    return () => hook.dispose();
  },
  server: async ({ directory }) => ({
    "tool.execute.before": async (input, output) => {
      if (input.tool !== "bash") return;
      const command = rewrite(input.sessionID, directory, output.args);
      if (command === undefined) return;
      output.args.command = command;
      if (output.args.background === true) output.args.background = false;
    },
  }),
};
