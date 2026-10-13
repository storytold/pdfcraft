#!/usr/bin/env python3
"""
Runs the 15-minute soak test on emulator-5554, saves every raw dumpsys meminfo
output as a timestamped file under target/soak/, and generates target/soak_meminfo.csv.
"""

import os
import subprocess
import time
import glob

ADB = os.path.expanduser("~/Library/Android/sdk/platform-tools/adb")
SERIAL = "emulator-5554"
PACKAGE = "com.storytold.pdfcraft.kit.test"
DUMP_DIR = "target/soak"

def main():
    os.makedirs(DUMP_DIR, exist_ok=True)
    # Clear previous dumps in target/soak
    for old_file in glob.glob(os.path.join(DUMP_DIR, "*.txt")):
        os.remove(old_file)

    print(f"Starting 15-minute emulator soak harness on {SERIAL}...")
    test_cmd = [
        "./gradlew", ":pdfcraft-kit:connectedAndroidTest",
        "-Pandroid.testInstrumentationRunnerArguments.class=com.storytold.pdfcraft.kit.PdfSoakInstrumentedTest"
    ]
    env = os.environ.copy()
    env["JAVA_HOME"] = "/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home"
    proc = subprocess.Popen(test_cmd, cwd="mobile/android", stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)

    print("Waiting for test process to appear on emulator...")
    app_pid = None
    for _ in range(60):
        try:
            pid_out = subprocess.check_output([ADB, "-s", SERIAL, "shell", "pidof", PACKAGE], text=True).strip()
            if pid_out:
                app_pid = pid_out
                print(f"Found running {PACKAGE} with PID {app_pid}")
                break
        except subprocess.CalledProcessError:
            pass
        if proc.poll() is not None:
            print("Gradle process terminated early!")
            out, _ = proc.communicate()
            print(out)
            return
        time.sleep(2)

    if not app_pid:
        print("Failed to detect package PID after 120s.")
        return

    start_time = time.time()
    total_duration_s = 900 # 15 minutes
    interval_s = 60
    sample_count = 0

    while True:
        elapsed_s = int(time.time() - start_time)
        now_ts = int(time.time())
        dump_path = os.path.join(DUMP_DIR, f"meminfo_{elapsed_s:04d}s_{now_ts}.txt")

        try:
            raw_out = subprocess.check_output([ADB, "-s", SERIAL, "shell", "dumpsys", "meminfo", PACKAGE], text=True)
            with open(dump_path, "w", encoding="utf-8") as f:
                f.write(raw_out)
            sample_count += 1
            print(f"[Sample {sample_count:02d} | {elapsed_s}s] Saved raw dumpsys to {dump_path}", flush=True)
        except Exception as e:
            print(f"Error dumping meminfo at {elapsed_s}s: {e}", flush=True)

        if elapsed_s >= total_duration_s:
            print(f"Reached {total_duration_s}s (15 minutes). Stopping soak monitoring.")
            break

        if proc.poll() is not None:
            print("Test process finished.")
            break

        time.sleep(interval_s)

    # If test process still running, wait for completion
    if proc.poll() is None:
        print("Waiting for Gradle test execution to finish...")
        proc.wait()

    print("Generating CSV and linear regression from raw dump files...")
    subprocess.run(["python3", "scripts/parse_soak.py", DUMP_DIR, "target/soak_meminfo.csv"], check=True)

if __name__ == "__main__":
    main()
