#!/bin/sh
python3 check_all.py | grep -q "all good" && ! grep -rqw calc lib
