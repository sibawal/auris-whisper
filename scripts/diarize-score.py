"""Оценка разделения по голосам (для CI и ручных проверок).

  diarize-score.py <разметка.json> <результат.json> [--max-err 5] [--speakers N]

Разметка — [[начало, конец, спикер], ...]; результат — вывод examples/diarize.
Ошибка — доля речи, отнесённой не тому спикеру после лучшего сопоставления
номеров; паузы между отрезками, как в приложении, достаются ближайшему.
С --max-err/--speakers завершается с кодом 1, если результат хуже."""
import sys, json, itertools
args = sys.argv[1:]
def opt(name):
    return float(args[args.index(name) + 1]) if name in args else None
truth = json.load(open(args[0])); res = json.load(open(args[1]))
step = 0.02
end = max(t[1] for t in truth)
def lab(turns, x, nearest=False):
    for a, b, s in turns:
        if a <= x < b: return s
    if nearest and turns:  # как speaker_at в приложении: ближайший отрезок
        return min(turns, key=lambda t: min(abs(t[0] - x), abs(t[1] - x)))[2]
    return None
def near_boundary(x, d=0.5):
    return any(abs(x - a) < d or abs(x - b) < d for a, b, _ in truth)
T, R, X = [], [], []
x = 0.0
while x < end:
    T.append(lab(truth, x)); R.append(lab(res["turns"], x, True)); X.append(x); x += step
ts = sorted({s for s in T if s is not None}); rs = sorted({s for s in R if s is not None})
best = None
for perm in itertools.permutations(rs, min(len(rs), len(ts))) if rs else [()]:
    m = dict(zip(perm, ts))
    err = sum(1 for t, r in zip(T, R) if t is not None and m.get(r) != t)
    if best is None or err < best[0]: best = (err, m)
m = best[1]
speech = sum(1 for t in T if t is not None)
miss = sum(1 for t, r in zip(T, R) if t is not None and r is None)
conf = [x for t, r, x in zip(T, R, X) if t is not None and r is not None and m.get(r) != t]
nb = sum(1 for x in conf if near_boundary(x))
print(f"err={100*best[0]/speech:5.1f}%  miss={100*miss/speech:4.1f}%  conf={100*len(conf)/speech:4.1f}% (у границ {100*nb/speech:4.1f}%)  found={len(rs)} (truth {len(ts)})  time={res['seconds']:.1f}s")

max_err, want = opt("--max-err"), opt("--speakers")
if (max_err is not None and 100 * best[0] / speech > max_err) or (want is not None and len(rs) != int(want)):
    sys.exit(1)
