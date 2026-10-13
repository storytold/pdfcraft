#!/usr/bin/env python3
"""
Parses raw `dumpsys meminfo` dump files from a soak test run, generates a CSV,
and computes linear regression slopes for memory categories.
"""

import sys
import os
import glob
import re
import csv

def parse_dumpsys_file(filepath):
    with open(filepath, "r", encoding="utf-8", errors="replace") as f:
        content = f.read()

    if "No process found" in content or not content.strip():
        return None

    native_heap = 0
    java_heap = 0
    code = 0
    graphics = 0
    total_pss = 0

    # Parse App Summary section first if present
    in_app_summary = False
    for line in content.splitlines():
        line_clean = line.strip()
        if "App Summary" in line_clean:
            in_app_summary = True
            continue
        if in_app_summary:
            if line_clean.startswith("Java Heap:"):
                nums = re.findall(r'\d+', line_clean)
                if nums: java_heap = int(nums[0])
            elif line_clean.startswith("Native Heap:"):
                nums = re.findall(r'\d+', line_clean)
                if nums: native_heap = int(nums[0])
            elif line_clean.startswith("Code:"):
                nums = re.findall(r'\d+', line_clean)
                if nums: code = int(nums[0])
            elif line_clean.startswith("Graphics:"):
                nums = re.findall(r'\d+', line_clean)
                if nums: graphics = int(nums[0])
            elif "TOTAL PSS:" in line_clean:
                nums = re.findall(r'\d+', line_clean)
                if nums: total_pss = int(nums[0])
            elif "Objects" in line_clean or line_clean.startswith("TOTAL RSS:"):
                in_app_summary = False

    # Fallback to table if App Summary was missing
    if total_pss == 0:
        for line in content.splitlines():
            line_clean = line.strip()
            parts = re.split(r'\s+', line_clean)
            if len(parts) >= 3 and parts[0] == "Native" and parts[1] == "Heap":
                try: native_heap = int(parts[2])
                except ValueError: pass
            elif len(parts) >= 2 and parts[0] == "TOTAL" and parts[1].isdigit():
                total_pss = int(parts[1])

    return {
        "native_heap_kb": native_heap,
        "java_heap_kb": java_heap,
        "code_kb": code,
        "graphics_kb": graphics,
        "total_pss_kb": total_pss
    }

def linear_regression_slope(xs, ys):
    if len(xs) < 2:
        return 0.0
    n = len(xs)
    mean_x = sum(xs) / n
    mean_y = sum(ys) / n
    numer = sum((xs[i] - mean_x) * (ys[i] - mean_y) for i in range(n))
    denom = sum((xs[i] - mean_x) ** 2 for i in range(n))
    if denom == 0:
        return 0.0
    return numer / denom

def main():
    dump_dir = sys.argv[1] if len(sys.argv) > 1 else "target/soak"
    out_csv = sys.argv[2] if len(sys.argv) > 2 else "target/soak_meminfo.csv"

    files = sorted(glob.glob(os.path.join(dump_dir, "meminfo_*.txt")))
    if not files:
        print(f"No meminfo_*.txt files found in {dump_dir}")
        sys.exit(1)

    records = []
    for f in files:
        # File format: meminfo_<seconds>s_<timestamp>.txt
        basename = os.path.basename(f)
        m = re.search(r'meminfo_(\d+)s_', basename)
        t_sec = float(m.group(1)) if m else 0.0
        data = parse_dumpsys_file(f)
        records.append((t_sec, data, basename))

    records.sort(key=lambda r: r[0])

    print(f"Parsed {len(records)} raw dump files from {dump_dir}. Writing to {out_csv}...")
    with open(out_csv, "w", newline="") as csvfile:
        writer = csv.writer(csvfile)
        writer.writerow(["time_s", "native_heap_kb", "java_heap_kb", "graphics_kb", "code_kb", "total_pss_kb", "status", "raw_dump_file"])
        for t_sec, data, fname in records:
            if data is not None:
                writer.writerow([t_sec, data["native_heap_kb"], data["java_heap_kb"], data["graphics_kb"], data["code_kb"], data["total_pss_kb"], "RUNNING", fname])
            else:
                writer.writerow([t_sec, "N/A", "N/A", "N/A", "N/A", "N/A", "PROCESS_EXITED", fname])

    valid_records = [r for r in records if r[1] is not None]
    if not valid_records:
        print("No valid active process records found.")
        return

    times_min = [r[0] / 60.0 for r in valid_records]
    native_heaps = [r[1]["native_heap_kb"] for r in valid_records]
    java_heaps = [r[1]["java_heap_kb"] for r in valid_records]
    total_psss = [r[1]["total_pss_kb"] for r in valid_records]

    slope_native = linear_regression_slope(times_min, native_heaps)
    slope_java = linear_regression_slope(times_min, java_heaps)
    slope_pss = linear_regression_slope(times_min, total_psss)

    print("\n--- LINEAR REGRESSION OVER RUN ---")
    print(f"Total raw dumpsys files: {len(records)}")
    print(f"Active samples used for regression: {len(valid_records)}")
    print(f"Active Duration: {times_min[-1]:.2f} minutes ({valid_records[-1][0]:.0f} s)")
    print(f"Start Total PSS: {total_psss[0]} KB ({total_psss[0]/1024:.2f} MiB)")
    print(f"End Total PSS:   {total_psss[-1]} KB ({total_psss[-1]/1024:.2f} MiB)")
    print(f"Start Native Heap: {native_heaps[0]} KB ({native_heaps[0]/1024:.2f} MiB)")
    print(f"End Native Heap:   {native_heaps[-1]} KB ({native_heaps[-1]/1024:.2f} MiB)")
    print(f"Native Heap Slope: {slope_native:+.2f} KB/min ({(slope_native/1024):+.4f} MiB/min)")
    print(f"Java Heap Slope:   {slope_java:+.2f} KB/min ({(slope_java/1024):+.4f} MiB/min)")
    print(f"Total PSS Slope:   {slope_pss:+.2f} KB/min ({(slope_pss/1024):+.4f} MiB/min)")

if __name__ == "__main__":
    main()
