#!/bin/sh
./run.sh >/dev/null && grep -q "total 42" logs/job.log
