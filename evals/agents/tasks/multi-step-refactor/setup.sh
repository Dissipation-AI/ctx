#!/bin/sh
mkdir -p lib
cat > lib/geometry.py <<'PY'
def calc(w, h):
    return w * h
PY
cat > lib/report.py <<'PY'
from lib.geometry import calc

def describe(w, h):
    return f"{w}x{h} = {calc(w, h)}"
PY
cat > lib/__init__.py <<'PY'
PY
cat > check_all.py <<'PY'
from lib.geometry import rectangle_area
from lib.report import describe
assert rectangle_area(3, 4) == 12
assert describe(2, 5) == "2x5 = 10"
print("all good")
PY
