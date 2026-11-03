#!/usr/bin/env bash
set -euo pipefail

echo "=== Searching for downloaded APKs ==="
DEBUG_APK=$(find apks -name "app-debug.apk" | head -n 1)
TEST_APK=$(find apks -name "app-debug-androidTest.apk" | head -n 1)

echo "Debug APK path: ${DEBUG_APK:-<none>}"
echo "Test APK path: ${TEST_APK:-<none>}"

if [ -z "${DEBUG_APK}" ] || [ ! -f "${DEBUG_APK}" ]; then
  echo "Error: Debug APK not found under apks directory"
  find apks -maxdepth 4 -ls 2>/dev/null || true
  exit 1
fi

if [ -z "${TEST_APK}" ] || [ ! -f "${TEST_APK}" ]; then
  echo "Error: Test APK not found under apks directory"
  find apks -maxdepth 4 -ls 2>/dev/null || true
  exit 1
fi

echo "=== Waiting for emulator to be fully ready ==="
adb wait-for-device
for i in $(seq 1 30); do
  boot_completed=$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)
  if [ "$boot_completed" = "1" ]; then
    echo "Emulator boot completed."
    break
  fi
  echo "Waiting for sys.boot_completed... ($i/30)"
  sleep 2
done

echo "=== Installing application under test ==="
adb install -r "$DEBUG_APK"

echo "=== Installing instrumentation test package ==="
adb install -r "$TEST_APK"

echo "=== Resolving installed instrumentation runner ==="
adb shell pm list instrumentation || true
RUNNER=$(adb shell pm list instrumentation 2>/dev/null | grep "pdfcraft" | sed -n 's/^instrumentation:\([^ ]*\).*/\1/p' | head -n 1 || true)
if [ -z "$RUNNER" ]; then
  RUNNER="io.github.dikshant363.pdfcraft.debug.test/androidx.test.runner.AndroidJUnitRunner"
fi
echo "Using instrumentation runner: $RUNNER"

echo "=== Executing all four instrumentation test classes ==="
CLASSES="io.github.dikshant363.pdfcraft.NativeEngineLoadingTest,io.github.dikshant363.pdfcraft.DocumentLifecycleTest,io.github.dikshant363.pdfcraft.ViewerOperationsTest,io.github.dikshant363.pdfcraft.AnnotationsAndMediaTest"

set +e
OUTPUT=$(adb shell am instrument -w -r \
  -e class "$CLASSES" \
  "$RUNNER" 2>&1)
EXIT_CODE=$?
set -e

echo "=== Instrumentation Runner Output ==="
echo "$OUTPUT"
echo "=== End Instrumentation Output (exit code: $EXIT_CODE) ==="

if [ $EXIT_CODE -ne 0 ]; then
  echo "Error: adb shell am instrument returned non-zero exit code: $EXIT_CODE"
  exit 1
fi

if echo "$OUTPUT" | grep -qE "FAILURES!!!|INSTRUMENTATION_FAILED|Failure in"; then
  echo "Error: Instrumentation tests reported failure."
  exit 1
fi

if ! echo "$OUTPUT" | grep -q "OK ("; then
  echo "Error: Instrumentation tests did not report successful completion."
  exit 1
fi

echo "All four instrumentation test classes executed and passed successfully on emulator!"
