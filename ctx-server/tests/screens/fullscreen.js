// A stand-in for htop-like full-screen programs: alternate screen, absolute cursor moves, colours,
// a scroll region, in-place updates, wide characters, and a final frame left on screen.
const w = (s) => process.stdout.write(s);
w("\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[H\x1b[2J");
w("\x1b[1;1H\x1b[7m PID  NAME            CPU%  MEM   \x1b[0m");
const procs = [["101", "server", 12.5], ["202", "worker", 3.1], ["303", "indexer", 0.4], ["404", "日本語ツール", 7.7]];
procs.forEach(([pid, name, cpu], i) => {
  w(`\x1b[${i + 2};1H ${pid.padEnd(5)}${name.padEnd(16 - (name === "日本語ツール" ? 6 : 0))}\x1b[${cpu > 10 ? "31" : "32"}m${cpu.toFixed(1).padStart(5)}\x1b[0m  ${(i + 1) * 10}M`);
});
// Update a cell in place, as a monitor does.
w("\x1b[3;24H\x1b[33m 9.9\x1b[0m");
// A scroll region at the bottom for a log pane.
w("\x1b[10;20r\x1b[20;1H");
for (let i = 1; i <= 14; i++) w(`\r\nlog ${i}`);
w("\x1b[r\x1b[22;1H\x1b[1mF1\x1b[0mHelp  \x1b[1mF10\x1b[0mQuit");
setTimeout(() => {}, 300);
