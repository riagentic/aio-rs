# The key-to-paint table from keys.ts rows (one per key), every session
# pooled: p50 / p95 of each clock, CPU per key the mean, and for each p50
# the 95% interval of aio-rs minus aio (a bootstrap of 2000 resamples):
# below 0 is aio-rs ahead, an interval across 0 is a draw.
#   python3 -I table.py <work>/keys.csv [aio label] [aio-rs label]
import csv, math, random, statistics, sys

rows = list(csv.reader(open(sys.argv[1])))
A, B = (sys.argv[2], sys.argv[3]) if len(sys.argv) > 3 else ("aio", "aio-rs")
KEYS = [("hold_down", "Hold ↓"), ("hold_up", "Hold ↑"), ("key_end", "End"), ("key_home", "Home"), ("key_tab", "Tab")]


def pct(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, max(0, math.ceil(p * len(a)) - 1))]


def col(label, metric, i):
    return [float(r[i]) for r in rows if r[0] == label and r[1] == metric and r[i] != ""]


def ci(a, b, rnd=random.Random(7)):
    d = sorted(pct(rnd.choices(b, k=len(b)), 0.5) - pct(rnd.choices(a, k=len(a)), 0.5) for _ in range(2000))
    return d[50], d[1949]


def sessions(label):
    return len({r[3] for r in rows if r[0] == label})


print(f"{A}: {sessions(A)} sessions, {B}: {sessions(B)} sessions")
print("| Key | n | aio stamp | aio-rs stamp | Δ p50 (95%) | aio ran | aio-rs ran | Δ p50 (95%) | CPU per key, aio / aio-rs |")
print("|---|---|---|---|---|---|---|---|---|")
for m, name in KEYS:
    out = [name, str(len(col(B, m, 4)))]
    for i in (4, 5):
        a, b = col(A, m, i), col(B, m, i)
        lo, hi = ci(a, b)
        out += [f"{pct(a, .5):.1f} / {pct(a, .95):.1f}", f"{pct(b, .5):.1f} / {pct(b, .95):.1f}", f"{pct(b, .5) - pct(a, .5):+.1f} ({lo:+.1f}, {hi:+.1f})"]
    cpu = [statistics.mean(col(L, m, 6)) for L in (A, B)]
    out.append(f"{cpu[0]:.1f} / {cpu[1]:.1f}")
    print("| " + " | ".join(out) + " |")
