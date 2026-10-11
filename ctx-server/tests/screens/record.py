"""Record a program's raw terminal output for golden-screen tests.

    python3 record.py <name> <keys-script> -- <program> [args…]

Runs the program in an 80x24 pseudo-terminal with TERM=xterm-256color, sends the keys script
(one step per line: text to type, `\\r` for Enter, `\\x1b` for Escape, `sleep 0.5`), and writes every
byte the program printed to <name>.bin. The test replays that through ctx's emulator and compares
the screen with <name>.txt (written on the first run; review it, then commit both).
"""
import fcntl, os, pty, select, struct, sys, termios, time

name, script = sys.argv[1], sys.argv[2]
argv = sys.argv[sys.argv.index("--") + 1:]
pid, fd = pty.fork()
if pid == 0:
    os.environ.update(TERM="xterm-256color", COLUMNS="80", LINES="24", LANG="en_US.UTF-8")
    os.execvp(argv[0], argv)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
out = bytearray()

def pump(seconds):
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            if not data:
                return
            out.extend(data)

pump(1.0)
for step in open(script).read().splitlines():
    if step.startswith("sleep "):
        pump(float(step.split()[1]))
        continue
    os.write(fd, step.encode().decode("unicode_escape").encode())
    pump(0.4)
pump(0.5)
try:
    os.kill(pid, 9)
except ProcessLookupError:
    pass
open(f"{name}.bin", "wb").write(out)
print(f"{name}.bin: {len(out)} bytes")
