# Android Performance Monitor

A fast, btop-style terminal UI for monitoring Android device and per-app performance in real time — over ADB, no root required.

![Screenshot](assets/screenshot.png)

## Features

- **Per-core CPU** — live usage bars and frequencies for every core
- **App CPU & threads** — the app's share of total CPU, plus a per-thread table (CPU% over the last interval, last core, CPU time)
- **Processes** — in device-wide mode the table lists the busiest processes (CPU%, MEM%, CPU time)
- **Memory** — device used / free / swap, and the app's PSS and heap size / allocated
- **Battery & Power** — level, charging state, deep and light doze state, app standby bucket
- **Thermal** — AP, SKIN, and battery temperatures with history
- **Network** — device-wide Wi-Fi / mobile RX / TX with a sparkline
- **Network capture** (optional [agent app](android-agent/README.md), no root) — each app's traffic, every connection with its domain name, DNS lookups, and a pcap file for Wireshark
- **Jobs & Alarms** — scheduled jobs and pending alarms for the selected app
- **Device mode** — monitor the whole device, or switch to a specific package
- **CSV recording** — one row per sample with `--output`
- **Flicker-free rendering** via `ratatui` with async ADB collectors

## Requirements

- [Rust](https://rustup.rs/) 1.78+ (1.82+ on Windows)
- [ADB](https://developer.android.com/tools/adb) in your `PATH`
- An Android device with **USB debugging enabled** (wired or over Wi-Fi)

## Install

```bash
git clone https://github.com/aabolfazl/android-perf.git
cd android-perf
cargo build --release
```

The binary will be at `target/release/android-perf-monitor`.

## Usage

```bash
# Pick device and package interactively
./target/release/android-perf-monitor

# Or pass everything via flags
./target/release/android-perf-monitor \
  --device <serial> \
  --package com.example.app \
  --interval 1 \
  --output samples.csv
```

### CLI options

| Flag | Description | Default |
|------|-------------|---------|
| `-d`, `--device <SERIAL>` | ADB device serial | first device |
| `-p`, `--package <NAME>`  | Package to track | device-wide |
| `-i`, `--interval <SECS>` | Sample interval in seconds (jobs and alarms use twice this) | `1` |
| `-o`, `--output <PATH>`   | Write one CSV row per sample | none |
| `--capture`               | Start the network capture agent on the phone | off |
| `--pcap <FILE>`           | Save every captured packet for Wireshark (implies `--capture`) | none |

The CSV columns are `timestamp, device, package, sys_cpu_pct, app_cpu_pct, mem_used_mb, app_pss_mb, battery_pct, battery_temp_c, cpu_temp_c, skin_temp_c, net_rx_bytes, net_tx_bytes`. App columns are empty in device-wide mode.

### Keybindings

| Key | Action |
|-----|--------|
| `q` / `Esc` / `Ctrl+C` | Quit (`Esc` closes an open picker first; in the package picker `q` is typed into the filter) |
| `Space` | Pause / resume sampling |
| `d` | Open device picker |
| `p` | Open package picker (type to filter) |
| `x` | Clear package selection (device-wide mode) |
| `f` | Toggle fullscreen on the threads panel |
| `c` | Start / stop the network capture |
| `n` | Toggle fullscreen connections view (needs the capture) |
| `↑` / `↓` | Navigate lists |
| `Enter` | Select |

### Reading the numbers

- **SYS** and **APP** CPU are shares of the whole CPU: all cores fully busy is 100%.
- **Thread** CPU% is relative to one core, like `top -H`: a thread that keeps one core busy shows 100%.
- Collector errors (no device, unauthorized device, adb missing) are shown in red in the footer.

## How it works

Each device gets one persistent `adb shell` session that streams metrics from `/proc`, `dumpsys`, and `top`; picking another device restarts it for the new serial. Each panel has its own async collector that pushes updates over an `mpsc` channel to the UI thread, which redraws at ~60fps using double-buffered `ratatui`. A debug log is written to `android-perf-monitor.log` in the current directory.

For network capture, the [agent app](android-agent/README.md) runs a local VPN on the phone and streams protobuf snapshots ([`proto/agent.proto`](proto/agent.proto)) over `adb forward`.

## License

MIT — see [LICENSE](LICENSE).
