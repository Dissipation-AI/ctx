#!/bin/sh
# Another agent started a migration and stopped half way. With ctx, its tab still exists and shows
# what it did; without ctx, only the files are left.
mkdir -p data out
for i in 1 2 3 4 5 6; do echo "record $i" > "data/$i.txt"; done
for i in 1 2 3; do tr a-z A-Z < "data/$i.txt" > "out/$i.txt"; done
if command -v ctx >/dev/null && ctx status >/dev/null 2>&1; then
  ctx open tab named "Agent A - migration" at "$PWD" >/dev/null
  ctx run in "Agent A - migration" 'for i in 1 2 3; do tr a-z A-Z < data/$i.txt > out/$i.txt; done; echo migrated 1-3 of 6, next: 4-6 the same way' >/dev/null
fi
