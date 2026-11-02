---
name: adbtop
description: Measure Android app performance on a connected phone or emulator with the adbtop CLI (CPU, threads, memory and leaks, frame jank, per-app network, battery and thermal). Use when asked to profile, benchmark or check the performance of an Android app, measure what an action costs (app start, login, scrolling), look for a memory leak or jank, see which hosts an app talks to, or add a performance regression check to CI.
---

# adbtop

`adbtop` reads performance data from an Android device over adb, without root. Its subcommands print JSON on stdout and progress on stderr, and never ask for input. Never run bare `adbtop`: that is the interactive TUI and needs a real terminal.

## Before measuring

- `adbtop devices` lists devices with their state. With more than one ready device, pass `-d SERIAL` to every command.
- `adbtop packages` lists running app packages; use it to find the package id. `-p PACKAGE` must be an installed package. Without `-p`, adbtop measures the whole device.
- A phone charging over USB reports charging current, not the app's drain; the report's `warnings` says so. Power results need Wi-Fi adb (`adb tcpip 5555`); suggest it, don't switch it yourself.

## Commands

| Goal | Command |
|---|---|
| Current values | `adbtop snapshot -p PKG` |
| Fixed-length recording | `adbtop record -p PKG --duration 60s -o report.json -q` |
| Fewer collectors, less load | `-m cpu,threads,memory,frames,network,power` (any subset) |
| One sample per interval | `--samples samples.jsonl` (JSON lines) or `--samples samples.csv` |
| Per-app network hosts | add `--capture` (see below) |
| CI gate | `--assert 'frames.janky_pct < 5'`, repeatable; exit code 3 if any check fails |

Without `-o`, the report goes to stdout. `-q` silences progress on stderr. `-i 500ms` changes the sample interval (default 1 s).

## Measure one action

```bash
adbtop record -p PKG -o /tmp/action.json -q &
sleep 5
adb shell input tap 540 1680
adbtop mark "tapped login"
sleep 15
adbtop stop
wait
```

`mark` and `stop` talk to the most recent recording. `--session ID` picks another; the id is printed on stderr when a recording starts without `-q`. `stop` makes the recording write its report and exit. In the report, `after_markers[i]` covers the `--after-marker` window (default 15 s) after each marker: `cpu_app_max`, `cpu_system_max`, `pss_mb_delta`, `rx_bytes`, `janky_frames`, `drain_ma_avg`.

For app start time, `adb shell am start -W -S -n PKG/ACTIVITY` prints `TotalTime`; put a marker right before it.

Frames only exist while the app draws. An idle screen or a playing video gives `frames.rendered: 0`, so drive the UI (`adb shell input swipe 540 1800 540 600 200`) while recording.

## Reading the report

`schema` is `adbtop/report@1`. Read `unsupported` and `warnings` first. `unsupported` names data this phone cannot provide, with the reason; those sections are missing, not zero. `warnings` are plain sentences with numbers, worth quoting to the user.

Numeric stats are objects `{avg, min, max, p95, last}`.

- **CPU**: `cpu.app_pct`, `cpu.app_kernel_pct` and `cpu.system_pct` are shares of all cores (100 = every core busy). `cpu.top_threads[].now/avg/peak` are % of one core. Don't compare the two scales directly. `cpu.clusters[]` has clock speeds and throttling caps.
- **Memory**: `memory.pss_mb` has `start`, `end`, `min`, `max`, `avg`, and `growth_per_min` only for recordings of at least 1 minute; a shorter recording can't show a leak. Leak signs: steady positive `growth_per_min`, a warning that the Java floor after GC rose, and `memory.objects` (`[start, end]` pairs) where `activities`, `app_contexts` or `views` keep growing while the user is back on the same screen. `memory.java_heap.gc_estimate` counts GCs.
- **Frames**: `frames.janky_pct`, `p50_ms`…`p99_ms`, `refresh_hz`, `fps`. `frames.causes`: `slow_ui_thread` is main-thread work, `slow_draw_commands` and `slow_bitmap_uploads` are rendering, `high_input_latency` is the input queue, `deadline_missed` is frames past their deadline.
- **Network**: `network.source` is `capture` (per app, from the agent: `top_hosts`, `failures` with reasons, `dns_failures`) or `device` (whole-phone byte counters only, no hosts).
- **Power**: `power.drain_ma` and `power.used_mah` only mean something on battery. `power.thermal.status`, `worst` and `max_c` per sensor; `power.doze`; `power.jobs`.

Assertions are `PATH OP NUMBER` with `<`, `<=`, `>`, `>=`, `==` or `!=` over these paths. A path that is not in the report fails its check, so assert `memory.pss_mb.growth_per_min` only on recordings of a minute or more.

## Per-app network capture

`--capture` (or `--pcap FILE`, which also saves every packet for Wireshark) starts a small agent app on the phone. It runs a local VPN that forwards traffic to where it was going and measures it; nothing goes to a server. HTTPS stays encrypted; you get hosts, bytes and failures, not contents. It stops when the recording ends.

1. `adbtop agent status` shows `installed`, `up_to_date` and `vpn_allowed`.
2. If the agent is missing or out of date, ask the user before `adbtop agent install`, since it installs an app on their phone. If that fails with `agent_signature_mismatch`, `adbtop agent install --replace` removes the old agent first; ask before that too.
3. If `vpn_allowed` is false, the first capture shows a VPN permission dialog on the phone. Tell the user to accept it, then record. Without it the report has a `capture: no data arrived` warning.

Another VPN on the phone is disconnected while the capture runs.

## Exit codes and errors

`0` ok, `1` error, `2` bad arguments, `3` an `--assert` failed, `4` device or app not found (or several devices without `-d`), `5` capture agent missing or not allowed. Errors are printed as `{"error": {"code": "...", "message": "..."}}` on stdout; the message says what to do next.

## Don't

- Don't force-stop, uninstall, or change Doze, battery or standby settings on the user's phone unless they asked.
- Keep recordings as long as the question needs, not longer: `dumpsys meminfo` costs the phone about 0.4 s every 2 s.
