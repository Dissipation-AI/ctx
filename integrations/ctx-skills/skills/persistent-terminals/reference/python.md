# Python, IPython, pdb, debugpy

## REPL with data loaded

```
ctx open tab named "Python - sales df" at ~/code/analysis
ctx run in "Python - sales df" "source ENV/bin/activate && PYTHON_BASIC_REPL=1 python -q" without waiting
ctx wait until "Python - sales df" is quiet for 1 seconds
ctx type 'import pandas as pd
df = pd.read_parquet("data/sales.parquet")
print(df.shape, "DONE" + "-1")' into "Python - sales df" then press enter
ctx wait until "Python - sales df" shows "DONE-1" within 5 minutes
```

- `PYTHON_BASIC_REPL=1` (3.13+): the new REPL takes a multi-line `type` as one paste and waits for an extra Enter. The basic REPL enters line by line.
- Indented blocks need a blank line before the next top-level statement.
- Large frames: `print(df.head(10).to_string())`, `df.describe()`, or `df.to_csv(f"{S}/x.csv")` and read the file. Never print a whole frame.
- `pd.set_option("display.width", 200, "display.max_columns", 50)` once, so tables don't wrap into unreadable columns.
- IPython: `ipython --simple-prompt --no-banner --colors=NoColor`. `%magic` and `!shell` lines go in their own `type`.
- A traceback doesn't kill the REPL. The state is still there, so fix and re-run only the failing line.

## pdb

Post-mortem on a crash, without editing code:

```
ctx run in "pdb - etl" "python -m pdb -c continue etl.py" without waiting
ctx wait until "pdb - etl" shows "(Pdb)" within 2 minutes
ctx type 'where
p locals().keys()
up
p row, total
!print("DONE" + "-1")' into "pdb - etl" then press enter
ctx wait until "pdb - etl" shows "DONE-1" within 20 seconds
ctx read last 40 lines in "pdb - etl"
```

- `python -m pdb -c continue script.py` runs to the first exception and stops there. `pytest --pdb -x` does the same for a failing test; `pytest --trace` stops at the start of each test.
- `b file.py:42, cond`, `c`, `n`, `s`, `r`, `until`, `pp expr`, `display expr`, `interact` (a full REPL in the frame; `ctrl-d` returns to pdb).
- `c`, `n` and `s` hand control to the program. End the batch there, wait for the next `(Pdb)` with a marker in the following batch, or wait until quiet.
- In post-mortem, `continue` or `step` restarts the program. Lines typed after it in the same batch reach the restarted run or the shell.
- `breakpoint()` in code drops into pdb inside whatever tab runs it, including a server tab.

## debugpy / attaching to something already running

`python -m debugpy --listen 5678 --wait-for-client app.py` in one tab. A second tab for the client or `pdb`-style inspection is usually simpler: add `breakpoint()` and run the server in its own tab so the breakpoint lands there, then drive it with `type`.

## Jupyter kernels

For notebook-like work without a notebook: `jupyter console --simple-prompt` in a tab behaves like IPython and keeps the kernel alive between calls.
