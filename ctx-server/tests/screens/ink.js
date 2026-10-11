// A stand-in for Ink/React TUIs (and Claude Code): redraws its frame in place with
// synchronized output (mode 2026), a hidden cursor, cursor-up and erase-line, like log-update.
const frames = 12;
let prev = 0;
process.stdout.write("\x1b[?25l");
for (let i = 0; i <= frames; i++) {
  const bar = "█".repeat(i) + "░".repeat(frames - i);
  const lines = [`\x1b[1;32m✓\x1b[0m Building ${bar} ${Math.round((i / frames) * 100)}%`, `  step ${i} of ${frames}`, `  \x1b[2mpress q to quit\x1b[0m`];
  let out = "\x1b[?2026h";
  if (prev) out += "\x1b[2K\x1b[1A".repeat(prev - 1) + "\x1b[2K\r";
  out += lines.join("\n");
  out += "\x1b[?2026l";
  process.stdout.write(out);
  prev = lines.length;
}
process.stdout.write("\n\x1b[?25hdone\n");
setTimeout(() => {}, 300);
