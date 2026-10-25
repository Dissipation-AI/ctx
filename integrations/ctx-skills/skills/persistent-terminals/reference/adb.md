# adb: Android, Fire TV, devices

## Prefer one-shot calls

`adb shell` with a command needs no persistent tab and returns the exit status:

```
adb devices -l
adb -s 10.0.0.127:5555 shell 'dumpsys window | grep mCurrentFocus'
adb shell pm list packages -3
adb shell am start -n com.example/.MainActivity
```

Set the device once in a tab instead of repeating `-s`: `export ANDROID_SERIAL=10.0.0.127:5555`. Put adb on the path once too, if it lives in a project folder: `export PATH=$PWD/platform-tools:$PATH`.

## logcat in its own tab

logcat never ends, so it gets its own tab, and other tabs drive the device:

```
ctx open tab named "Logcat - firetv"
ctx run in "Logcat - firetv" "adb logcat -c && adb logcat -v time '*:W' MyApp:V" without waiting
ctx run in "Build - app" "./build.sh && adb install -r out/app.apk && adb shell am start -n com.lumen.tv/.Main"
ctx wait until "Logcat - firetv" shows "FATAL EXCEPTION" within 30 seconds
ctx read last 60 lines in "Logcat - firetv"
```

- `adb logcat -c` clears the buffer first, so old crashes don't match.
- Filter at the source (`'*:W'`, `MyApp:V`, `--pid=$(adb shell pidof -s com.pkg)`). Grepping a firehose after the fact costs tokens.
- For a one-off snapshot: `adb logcat -d -t 500 | grep -E "ANR|FATAL"`.

## Interactive device shell

When you'll poke around a lot (`cd`, `ls`, `getprop`, `settings`), a persistent `adb shell` saves the connection setup each time:

```
ctx run in "adb - firetv" "adb shell" without waiting
ctx type 'getprop ro.build.version.release; echo DONE-$((1+1))' into "adb - firetv" then press enter
ctx wait until "adb - firetv" shows "DONE-2" within 15 seconds
```

The device's shell (mksh/toybox) has no ctx shell-integration marks, so use `type` + marker there, not `run`.

## Connection drops

`error: device offline` / `no devices/emulators found`: `adb connect <ip>:5555` (network devices), then check `adb devices`. A device prompting "Allow USB debugging?" or showing a pairing code needs the user.
