#!/usr/bin/env python3
"""Summarize compare.py sessions without treating timing batches as independent runs.

Usage: python3 summarize.py RESULT_DIRECTORY [--output-prefix PATH]
Writes JSON, CSV, and text summaries; the text summary is also printed.
"""

import argparse
from collections import defaultdict
import csv
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys


# Two-sided 95% Student t critical values, degrees of freedom 1 through 30.
T975 = (
    12.706205, 4.302653, 3.182446, 2.776445, 2.570582, 2.446912,
    2.364624, 2.306004, 2.262157, 2.228139, 2.200985, 2.178813,
    2.160369, 2.144787, 2.131450, 2.119905, 2.109816, 2.100922,
    2.093024, 2.085963, 2.079614, 2.073873, 2.068658, 2.063899,
    2.059539, 2.055529, 2.051831, 2.048407, 2.045230, 2.042272,
)
CPU_FIELDS = ("user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal")
METHOD = (
    "Each session estimate is median(log(candidate/baseline)) across balanced blocks. "
    "Each block time averages its two observations per slot. The aggregate is "
    "exp(mean(session log estimates)); its exploratory two-sided 95% t interval "
    "uses independent Chromium sessions, never individual batches. The default "
    "n=4 is small and does not establish equivalence or a reliable population interval."
)
AA_NOTE = (
    "A/A is baseline slot 1 / baseline slot 0: separate instances of the identical build, "
    "loaded second / first. ABBA/BAAB timing order alternates; this is an instance/order "
    "control, not a code-change effect."
)
HOST_NOTE = (
    "Host-wide /proc/stat deltas exclude guest and guest_nice (already included in "
    "user/nice). Busy excludes idle and iowait but includes steal; executing also "
    "excludes steal. Case windows include correctness, warmup and all measured modes; "
    "they do not isolate the browser or one mode. Load ranges use endpoint snapshots."
)


def positive(value, label):
    value = float(value)
    if not math.isfinite(value) or value <= 0:
        raise ValueError(f"{label} must be finite and positive")
    return value


def host_window(before, after):
    ticks = {field: after["cpu_ticks"].get(field, 0) - before["cpu_ticks"].get(field, 0)
             for field in CPU_FIELDS}
    # Do not silently clamp inconsistent counters and invent utilization.
    valid = all(value >= 0 for value in ticks.values()) and sum(ticks.values()) > 0
    return {"delta_ticks": ticks if valid else None,
            "load_samples": [before["load_average"], after["load_average"]]}


def host_summary(windows):
    valid = [window["delta_ticks"] for window in windows if window["delta_ticks"] is not None]
    ticks = {field: sum(row[field] for row in valid) for field in CPU_FIELDS}
    total = sum(ticks.values())
    busy = total - ticks["idle"] - ticks["iowait"]
    loads = [sample for window in windows for sample in window["load_samples"]]
    return {
        "windows": len(windows), "valid_cpu_windows": len(valid),
        "delta_ticks": ticks,
        "busy_percent": 100 * busy / total if total else None,
        "executing_percent": 100 * (busy - ticks["steal"]) / total if total else None,
        "steal_percent": 100 * ticks["steal"] / total if total else None,
        "iowait_percent": 100 * ticks["iowait"] / total if total else None,
        "load_ranges": {name: [min(row[index] for row in loads), max(row[index] for row in loads)]
                        for index, name in enumerate(("1m", "5m", "15m"))},
    }


def summarize_row(session, measurement, row, filename):
    plan = session["plan"]
    pair = measurement["pair"]
    comparison = plan["comparison"]
    baseline = plan.get("baseline", "scalar")
    if pair != plan["pair"] or len(pair) != 2:
        raise ValueError(f"{filename}: inconsistent pair")
    if comparison == "aa":
        if pair != [baseline, baseline]:
            raise ValueError(f"{filename}: A/A must use two baseline instances")
        baseline_slot, candidate_slot = 0, 1
    elif comparison != baseline and sorted(pair) == sorted([baseline, comparison]):
        baseline_slot, candidate_slot = pair.index(baseline), pair.index(comparison)
    else:
        raise ValueError(f"{filename}: unsupported comparison/pair {comparison}/{pair}")
    logs = []
    for block in row["blocks"]:
        times = [positive(block[key], f"{filename} block {key}") for key in ("a_us", "b_us")]
        logs.append(math.log(times[candidate_slot]) - math.log(times[baseline_slot]))
    if not logs:
        raise ValueError(f"{filename}: no timing blocks")
    medians = [statistics.median(positive(value, f"{filename} sample")
                                 for value in row[key]["samples_us"]) for key in ("a", "b")]
    estimate = statistics.median(logs)
    window = host_window(measurement["host_before"], measurement["host_after"])
    result = {
        "file": filename, "session": plan["session"], "round": plan["round"],
        "pair": pair, "baseline": baseline, "baseline_slot": baseline_slot, "candidate_slot": candidate_slot,
        "initial_block_order": measurement["initial_block_order"],
        "mode_order": measurement["mode_order"], "iterations": row["iterations"],
        "blocks": len(logs), "log_ratio_estimate": estimate,
        "ratio": math.exp(estimate), "change_percent": 100 * math.expm1(estimate),
        "baseline_median_us": medians[baseline_slot], "candidate_median_us": medians[candidate_slot],
        "host": host_summary([window]), "host_window": window,
    }
    if baseline == "scalar":
        # Keep historical result consumers compatible without mislabeling direct SIMD runs.
        result.update(scalar_slot=baseline_slot, scalar_median_us=medians[baseline_slot])
    return result


def aggregate(key, rows):
    comparison, mode, case = key
    baseline = rows[0]["baseline"]
    if any(row["baseline"] != baseline for row in rows):
        raise ValueError(f"{key}: cannot pool different baselines")
    logs = [row["log_ratio_estimate"] for row in rows]
    estimate = statistics.mean(logs)
    count = len(logs)
    interval = None
    if count > 1:
        degrees = count - 1
        # For more than 31 sessions, retain the df=30 critical value conservatively.
        critical_degrees = min(degrees, len(T975))
        margin = T975[critical_degrees - 1] * statistics.stdev(logs) / math.sqrt(count)
        interval = {"ratio": [math.exp(estimate - margin), math.exp(estimate + margin)],
                    "change_percent": [100 * math.expm1(estimate - margin), 100 * math.expm1(estimate + margin)],
                    "degrees_of_freedom": degrees, "critical_value_df": critical_degrees,
                    "critical_value": T975[critical_degrees - 1], "exploratory": True}
    result = {
        "comparison": comparison, "mode": mode, "case": case, "n_sessions": count,
        "baseline": baseline,
        "ratio_meaning": f"{baseline} slot 1 / {baseline} slot 0" if comparison == "aa" else f"{comparison} / {baseline}",
        "equal_work": not (comparison == "fused" and mode == "prepare"),
        "ratio": math.exp(estimate), "change_percent": 100 * math.expm1(estimate),
        "exploratory_ci95": interval,
        "baseline_median_us": statistics.median(row["baseline_median_us"] for row in rows),
        "candidate_median_us": statistics.median(row["candidate_median_us"] for row in rows),
        "host": host_summary([row["host_window"] for row in rows]),
        "sessions": [{name: value for name, value in row.items() if name != "host_window"} for row in rows],
    }
    if baseline == "scalar":
        result["scalar_median_us"] = result["baseline_median_us"]
    return result


def summarize(directory):
    groups = defaultdict(list)
    input_hashes = {}
    session_windows = []
    sessions = set()
    warnings = []
    for path in sorted(directory.glob("session-*.json")):
        data = path.read_bytes()
        session = json.loads(data)
        plan = session["plan"]
        if plan["kind"] != "timing" or session.get("browser_errors"):
            raise ValueError(f"{path.name}: not a successful timing session")
        session_id = plan["session"]
        if session_id in sessions:
            raise ValueError(f"duplicate session id {session_id}")
        sessions.add(session_id)
        input_hashes[path.name] = hashlib.sha256(data).hexdigest()
        session_windows.append(host_window(session["host_before"], session["host_after"]))
        seen = set()
        for measurement in session["measurements"]:
            if not measurement["final_rgba_equal"]:
                raise ValueError(f"{path.name}: final canvas check failed")
            for row in measurement["results"]:
                key = (plan["comparison"], row["mode"], measurement["fixture"]["id"])
                if key in seen:
                    raise ValueError(f"{path.name}: duplicate comparison/mode/case {key}")
                seen.add(key)
                groups[key].append(summarize_row(session, measurement, row, path.name))
    if not groups:
        raise ValueError("no completed session-*.json timing data")
    expected_sessions = None
    experiment_path = directory / "experiment.json"
    if experiment_path.is_file():
        data = experiment_path.read_bytes()
        input_hashes[experiment_path.name] = hashlib.sha256(data).hexdigest()
        expected_sessions = len(json.loads(data)["schedule"])
        if expected_sessions != len(sessions):
            warnings.append(f"Partial experiment: {len(sessions)} of {expected_sessions} planned sessions.")
    results = [aggregate(key, rows) for key, rows in sorted(groups.items())]
    counts = sorted({row["n_sessions"] for row in results})
    if counts != [4]:
        warnings.append(f"Observed independent session counts per group: {counts}; default design uses 4.")
    if any(row["host"]["valid_cpu_windows"] != row["n_sessions"] for row in results):
        warnings.append("Some CPU counter windows were invalid; they were excluded from utilization estimates.")
    return {"schema_version": 2, "method": METHOD, "aa_note": AA_NOTE, "host_note": HOST_NOTE,
            "absolute_time_method": "median of independent session raw-sample medians; not the paired-ratio estimator",
            "prepare_note": "prepare is raw extraction, except historical fused also includes alpha; select using draw",
            "timing_scope": "synchronous submission; excludes compositor completion and RDP decoding",
            "input_sha256": input_hashes, "n_sessions": len(sessions),
            "planned_sessions": expected_sessions, "warnings": warnings,
            "host": host_summary(session_windows), "results": results}


def text_summary(result):
    lines = [METHOD, AA_NOTE, HOST_NOTE,
             "Times below: medians of session medians. Ratios below 1 mean candidate is faster.",
             "prepare includes alpha only for historical fused; select implementations using complete draw work."]
    lines.extend(f"WARNING: {warning}" for warning in result["warnings"])
    lines.append("comparison mode case n baseline_us candidate_us ratio change% exploratory_95%_change per_session_ratios")
    for row in result["results"]:
        interval = row["exploratory_ci95"]
        bounds = "unavailable" if interval is None else "/".join(f"{value:+.2f}%" for value in interval["change_percent"])
        ratios = ",".join(f"{session['ratio']:.4f}" for session in row["sessions"])
        lines.append(f"{row['comparison']} {row['mode']} {row['case']} {row['n_sessions']} "
                     f"{row['baseline_median_us']:.3f} {row['candidate_median_us']:.3f} {row['ratio']:.4f} "
                     f"{row['change_percent']:+.2f}% [{bounds}] {ratios}")
    host = result["host"]
    busy = "unavailable" if host["busy_percent"] is None else f"{host['busy_percent']:.1f}%"
    loads = ", ".join(f"{name} {low:.2f}-{high:.2f}" for name, (low, high) in host["load_ranges"].items())
    lines.append(f"Host CPU busy (includes steal): {busy}; load endpoint ranges: {loads}")
    return "\n".join(lines) + "\n"


def write_csv(path, results):
    baseline_field = "scalar_median_us" if all(row["baseline"] == "scalar" for row in results) else "baseline_median_us"
    fields = ["comparison", "mode", "case", "n_sessions", "equal_work", "ratio", "change_percent",
              "exploratory_ci95_low_percent", "exploratory_ci95_high_percent", baseline_field,
              "candidate_median_us", "host_cpu_busy_percent", "host_cpu_steal_percent",
              "host_load_1m_min", "host_load_1m_max", "session_ratios"]
    with path.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=fields, lineterminator="\n")
        writer.writeheader()
        for row in results:
            interval = row["exploratory_ci95"]
            bounds = interval["change_percent"] if interval else [None, None]
            values = {field: row[field] for field in fields if field in row}
            values.update(exploratory_ci95_low_percent=bounds[0], exploratory_ci95_high_percent=bounds[1],
                          host_cpu_busy_percent=row["host"]["busy_percent"],
                          host_cpu_steal_percent=row["host"]["steal_percent"],
                          host_load_1m_min=row["host"]["load_ranges"]["1m"][0],
                          host_load_1m_max=row["host"]["load_ranges"]["1m"][1],
                          session_ratios=";".join(str(session["ratio"]) for session in row["sessions"]))
            writer.writerow(values)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result_directory", type=Path)
    parser.add_argument("--output-prefix", type=Path, help="default: RESULT_DIRECTORY/summary")
    args = parser.parse_args()
    result = summarize(args.result_directory)
    prefix = args.output_prefix or args.result_directory / "summary"
    text = text_summary(result)
    Path(f"{prefix}.json").write_text(json.dumps(result, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    write_csv(Path(f"{prefix}.csv"), result["results"])
    Path(f"{prefix}.txt").write_text(text, encoding="utf-8")
    print(text, end="")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, TypeError, ValueError, OSError, statistics.StatisticsError) as error:
        print(f"error: {type(error).__name__}: {error}", file=sys.stderr)
        raise SystemExit(1) from error
