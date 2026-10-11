#!/bin/sh
for i in 1 2 3 4 5 6; do [ "$(cat out/$i.txt 2>/dev/null)" = "RECORD $i" ] || exit 1; done
