#!/bin/sh
curl -s -m 3 http://127.0.0.1:8765/ | grep -q "hello from the eval app"
st=$?
pkill -f "python3 app.py" 2>/dev/null
exit $st
