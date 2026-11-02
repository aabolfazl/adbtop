# Android Performance Monitor

A terminal performance monitor for Android apps and devices, over ADB, without root. It has two modes:

- **Interactive TUI:** six pages of live charts (CPU, memory, frames, network, power) with markers, themes and settings.
- **Headless CLI:** records the same data and writes JSON, so scripts, CI jobs and AI agents can use it.

![Screenshot](assets/screenshot.png)

## Requirements

- [Rust](https://rustup.rs/) 1.78+
- [ADB](https://developer.android.com/tools/adb) on your `PATH`
- An Android device with **USB debugging** enabled (Android 6+; frame stats, thermal status and memory pressure need newer versions)

```bash
git clone https://github.com/aabolfazl/android-perf.git
cd android-perf
cargo build --release
```

The binary is `target/release/android-perf-monitor`.

## TUI

```bash
android-perf-monitor                      # pick a device and an app inside the UI
android-perf-monitor -p com.example.app   # start on one app
android-perf-monitor --theme "tokyo night"
```

| Page | What it shows |
|---|---|
| 1 Overview | One tile per page, each with a chart and the key numbers |
| 2 CPU | App vs system CPU, the app's kernel time, a per-core heatmap grouped by cluster (little/mid/big/prime), cluster clock speeds and caps, threads with state, average, peak and 2-minute history, thread pools |
| 3 Memory | App-owned PSS by category over time, Java heap size vs allocated (GC drops), Activities/Views/AppContexts/Binders with their change since start, SQLite databases and statement-cache hit rate, device RAM and swap |
| 4 Frames | Time of every frame against the vsync budget, FPS, jank %, percentiles, jank causes and distribution (from `dumpsys gfxinfo framestats`) |
| 5 Network | Received/sent per second; with the [capture agent](android-agent/README.md): top hosts, DNS lookups and every connection with where its name came from (SNI/DNS/HTTP) and why it failed |
| 6 Power | Battery current (drain in mA and W), mAh used this session, all thermal sensors, throttling status, doze and standby bucket, jobs and alarms |

Every panel says **not supported on this device** (with the reason) when the phone can't provide that data.

Changes the TUI makes to the phone (forced Doze, fake unplug, a different standby bucket) show as a ⚠ badge in the status bar. They are undone when you quit or switch devices.

The network capture (VPN) stops whenever the TUI exits: on `q` or Ctrl+C, when the terminal is closed, on SIGTERM, and after a crash. If the monitor disappears without a chance to clean up (`kill -9`, cable unplugged, laptop asleep), the agent stops itself once no monitor has been connected for 30 s.

| Key | Action |
|---|---|
| `1`–`6`, `Tab`, `Shift+Tab` | Switch pages (`Esc` goes back to Overview) |
| `p` / `x` | Pick an app / measure the whole device |
| `d` | Pick a device |
| `m` / `M` | Drop a marker on every chart now / with a label |
| `w` | Chart window: 1, 2, 5, 15, 30 minutes |
| `Space` | Pause |
| `c` | Start or stop the network capture agent |
| `a` | Actions: launch, cold restart (measures the start time and adds a marker), force stop, reset frame stats, Doze, standby bucket, fake unplug |
| `:` | Run any adb command on the selected device; the output streams into a scrollable window (`Esc` stops long commands like `logcat`) |
| `z` / `b` / `u` | Power page: force Doze on/off, next standby bucket, fake unplug |
| `r` | Frames page: reset frame stats |
| `s`, `g`, `/`, `↑↓` | Sort, group thread pools, filter, scroll the table on this page |
| `e` | Save a JSON report of this session |
| `t` / `T` | Next / previous theme |
| `,` | Settings |
| `?` | Help |
| `q`, `Ctrl+C` | Quit (stops the capture agent) |

### Settings and themes

Settings live in `~/.config/android-perf-monitor/config.toml` (or under `$XDG_CONFIG_HOME`). Edit them with `,` in the TUI or by hand. Command-line flags override them.

| Setting | What it changes |
|---|---|
| Theme, graph style, key hints, start page | Look, braille or block charts (for fonts without braille), footer shortcuts, the page shown at start |
| CPU % | App and system CPU as a share of all cores, or top-style % of one core |
| Temperature | °C or °F |
| Chart window, memory chart, network scale | Default time span; stacked app-owned memory or full PSS with shared Code/System; √ or linear network axis |
| Hide idle threads | Drop threads that never used CPU from the tables |
| Interval, memory interval, keep history | Sampling rates and how long history is kept |
| Frames, network capture, battery & thermal, process list | Turn collectors off to reduce load on the phone |
| Reopen last app, report folder, export on quit | Start on the last app; where `e` and export-on-quit save JSON reports |

Built-in themes: Gruvbox Dark (default), Catppuccin Mocha, Tokyo Night, Nord, Dracula, One Dark, Solarized Dark, Kanagawa, Everforest Dark, Monokai, GitHub Dark, Ayu Dark, Night Owl, Catppuccin Latte (light). To add your own:

```toml
[themes.mine]
base = "nord"
bg = "#101418"
accent = "#ffcc66"
```

The roles you can set are `bg`, `hi`, `fg`, `dim`, `dim2`, `gray`, `track`, `sel`, `red`, `green`, `yellow`, `blue`, `purple`, `aqua` and `orange`; any you leave out come from `base`.

## Headless: scripts, CI and AI agents

Every command prints JSON on stdout; progress messages go to stderr. Nothing asks for input.

```bash
# record 60 s of everything for one app, then print the report
android-perf-monitor record -p com.example.app --duration 60s > report.json

# only some metrics, report to a file, one JSON line per second on stdout
android-perf-monitor record -p com.example.app -m cpu,memory -o report.json --samples -

# samples as CSV
android-perf-monitor record -p com.example.app --duration 5m --samples samples.csv -o report.json

# current values, one sample
android-perf-monitor snapshot -p com.example.app

android-perf-monitor devices
android-perf-monitor packages
android-perf-monitor themes
```

Measuring one action, e.g. from an agent that drives the phone:

```bash
android-perf-monitor record -p com.example.app -o login.json &
adb shell input tap 540 1680
android-perf-monitor mark "tapped login"
sleep 15
android-perf-monitor stop
```

The report's `after_markers` section summarises what happened in the seconds after each marker (`--after-marker 15s` by default): peak app CPU, PSS change, bytes downloaded, janky frames and battery drain.

Regression checks: each `--assert` is `PATH OP NUMBER` over the report's JSON, with `<`, `<=`, `>`, `>=`, `==` or `!=`:

```bash
android-perf-monitor record -p com.example.app --duration 2m \
  --assert 'frames.janky_pct < 5' \
  --assert 'memory.pss_mb.growth_per_min < 0.5' \
  --assert 'network.failed == 0' \
  -o ci.json
```

### Report

`"schema": "android-perf-monitor/report@1"`. The main sections:

| Section | Contents |
|---|---|
| `device`, `app`, `session` | Model, Android version, CPU clusters; package, pid, state and restarts; start time, duration, samples |
| `unsupported` | Data sources this device can't provide, with the reason |
| `cpu` | `app_pct`, `app_kernel_pct`, `system_pct` (avg/min/max/p95/last), per-core averages, cluster clock speeds and caps, top threads |
| `memory` | `pss_mb` with `growth_per_min`, PSS by category, Java heap and a GC estimate, object counts `[start, end]`, SQLite, device RAM and memory pressure |
| `frames` | Frames rendered and janky, `janky_pct`, p50–p99, GPU percentiles, refresh rate, FPS, jank causes |
| `network` | Source (`capture` or `device`), bytes, rates, connections, failures, top hosts, DNS failures |
| `power` | Battery level, status and plugged state, `drain_ma`, `used_mah`, thermal status and peak temperature per sensor, doze, jobs |
| `markers`, `after_markers` | Markers and what followed each one |
| `warnings` | Plain sentences with numbers, e.g. a PSS growth trend, AppContexts growing while Activities stay flat, failed hosts, throttling, a plugged-in phone |
| `assertions` | Each check with its value and whether it passed |

CPU percentages for the app and the system are shares of all cores (100 = every core busy); thread values are % of one core.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | OK |
| 1 | Error (adb missing, cannot write the report, no running recording for `mark`/`stop`) |
| 2 | Bad arguments |
| 3 | An `--assert` failed |
| 4 | Device or app not found, device not authorized, or more than one device without `-d` |
| 5 | Capture agent missing or not allowed |

Errors are printed as `{"error": {"code": "...", "message": "..."}}`.

## Notes

- While the phone is plugged in it charges, so battery current is not your app's drain. Measure power over Wi-Fi adb (`adb tcpip 5555`).
- Linux cuts thread names to 15 characters.
- `dumpsys meminfo` takes about 0.4 s on the phone, so app memory is sampled every 2 s by default.
- Android keeps only the last ~120 frames per window, so at 120 Hz a few frames can be missed between samples; the jank percentages and percentiles come from Android's own counters and are exact.

## How it works

The fast metrics (`/proc` CPU, threads, memory, network, CPU clocks) come from a single `grep` over all the files each interval, through a persistent `adb shell`. `dumpsys meminfo`, `dumpsys gfxinfo` and the slower services each run on their own persistent shell, so a slow command never delays the others. At startup a probe reads the Android version, the CPU clusters and which services exist; anything missing is marked as unsupported. All of this feeds one session that keeps the history and computes the statistics; the TUI, the JSON report and the per-sample stream all read from it.

For network capture, the [agent app](android-agent/README.md) runs a local VPN on the phone and streams protobuf snapshots ([`proto/agent.proto`](proto/agent.proto)) over `adb forward`.

## License

MIT, see [LICENSE](LICENSE).
