#!/usr/bin/env python3
"""SQLite 多进程并发写实测: 本机盘(/tmp) vs Windows 挂载盘(/mnt/d), WAL 模式。

3 个进程 × 200 次独立事务 INSERT, 记录失败(SQLITE_BUSY)次数。
对照组: busy_timeout=0 vs 5000ms。
"""
import json
import multiprocessing as mp
import os
import sqlite3
import sys
import time

N_PROC = 3
N_INS = 200


def worker(db, busy_ms, out):
    conn = sqlite3.connect(db, timeout=busy_ms / 1000.0, isolation_level=None)
    conn.execute(f"PRAGMA busy_timeout={busy_ms}")
    conn.execute("PRAGMA journal_mode=WAL")
    ok = 0
    busy = 0
    other = 0
    t0 = time.time()
    for i in range(N_INS):
        try:
            conn.execute(
                "INSERT INTO t(ts, who, val) VALUES (?,?,?)",
                (time.time(), os.getpid(), i),
            )
            ok += 1
        except sqlite3.OperationalError as e:
            if "lock" in str(e).lower():
                busy += 1
            else:
                other += 1
        except Exception:
            other += 1
    out.append({"pid": os.getpid(), "ok": ok, "busy": busy, "other": other,
                "secs": round(time.time() - t0, 2)})


def trial(db, busy_ms):
    if os.path.exists(db):
        os.remove(db)
    for suffix in ("-wal", "-shm"):
        if os.path.exists(db + suffix):
            os.remove(db + suffix)
    conn = sqlite3.connect(db)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("CREATE TABLE t(ts REAL, who INT, val INT)")
    conn.commit()
    conn.close()

    mgr = mp.Manager()
    out = mgr.list()
    ps = [mp.Process(target=worker, args=(db, busy_ms, out)) for _ in range(N_PROC)]
    t0 = time.time()
    for p in ps:
        p.start()
    for p in ps:
        p.join()
    wall = round(time.time() - t0, 2)

    conn = sqlite3.connect(db)
    total = conn.execute("SELECT COUNT(*) FROM t").fetchone()[0]
    jm = conn.execute("PRAGMA journal_mode").fetchone()[0]
    conn.close()
    return {"db": db, "busy_timeout_ms": busy_ms, "journal_mode": jm,
            "inserted_expected": N_PROC * N_INS, "inserted_actual": total,
            "wall_secs": wall, "workers": list(out)}


if __name__ == "__main__":
    targets = [("/tmp/locus_probe/local.db", "本机盘 /tmp"),
               ("/mnt/d/locus_probe/wsl_drvfs.db", "Windows 盘 /mnt/d (drvfs)")]
    results = []
    for path, label in targets:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        for busy in (0, 5000):
            r = trial(path, busy)
            r["label"] = label
            results.append(r)
            print(json.dumps(r, ensure_ascii=False))
            sys.stdout.flush()
    print("DONE")
