#!/bin/sh
# A job that fails; the reason is only in its log.
mkdir -p config logs
cat > config/job.ini <<'INI'
[job]
input = data/input.csv
retries = three
INI
mkdir -p data
printf 'id,value\n1,10\n2,32\n' > data/input.csv
cat > run.sh <<'SH'
#!/bin/sh
python3 - <<'PY' >> logs/job.log 2>&1
import configparser, csv, sys
c = configparser.ConfigParser(); c.read("config/job.ini")
print("starting job")
try:
    retries = int(c["job"]["retries"])
except ValueError as e:
    print(f"ERROR config/job.ini [job] retries must be a whole number: {e}")
    sys.exit(3)
rows = list(csv.DictReader(open(c["job"]["input"])))
print("total", sum(int(r["value"]) for r in rows))
PY
st=$?
[ $st -eq 0 ] && echo "job finished" || echo "job failed (see logs/job.log)"
exit $st
SH
chmod +x run.sh
for i in $(seq 1 200); do echo "2026-10-11 10:00:$i INFO heartbeat $i" >> logs/job.log; done
