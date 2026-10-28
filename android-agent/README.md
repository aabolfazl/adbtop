# Android Perf Agent

A small Android app that lets the terminal UI see every app's network traffic, without root.

It works like a local VPN, the same way [PCAPdroid](https://github.com/emanuele-f/PCAPdroid) does:
Android routes the apps' traffic into the agent, which forwards it to where it was going over
normal sockets and measures it on the way. **Nothing is sent to a VPN server**; the traffic
leaves the phone exactly as before.

What it reports to the terminal UI, once a second:

- Traffic per app (bytes received and sent)
- Every TCP and UDP connection: app, destination, host name, bytes, state, why it failed
- Host names from DNS answers, the TLS server name (SNI) and the HTTP `Host` header
- Recent DNS lookups
- Optionally, every packet as a pcap stream that opens in Wireshark

## Build and install

Needs the Android SDK (Android Studio, or the command-line tools with `ANDROID_HOME` set) and JDK 17+.

```bash
cd android-agent
./gradlew :app:assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

Requires Android 10 or newer.

## Use it from the terminal UI

```bash
android-perf-monitor --capture -p com.example.app
android-perf-monitor --pcap capture.pcap      # also save every packet for Wireshark
```

Or press `c` to start and stop the capture, and `n` for the connections view. The first time,
the phone asks for VPN permission; accept it once. The capture stops when the monitor quits.

## Use it by hand

```bash
# Start (optionally only for one app) and stop
adb shell am start -n io.github.aabolfazl.perfagent/.ControlActivity --es cmd start --es app com.example.app
adb shell am start -n io.github.aabolfazl.perfagent/.ControlActivity --es cmd stop

# Live pcap into Wireshark
adb forward tcp:5555 localabstract:android-perf-agent-pcap
nc localhost 5555 | wireshark -k -i -
```

The snapshot stream is on `localabstract:android-perf-agent`: length-delimited protobuf
`Snapshot` messages, defined in [`../proto/agent.proto`](../proto/agent.proto). Only adb (the
shell user) and root can connect to either socket, so other apps on the phone can't read it.

## Limitations

- **One VPN at a time.** Another VPN app on the phone is disconnected while the capture runs.
- **IPv4 only.** Apps are kept on IPv4 while capturing (Android blocks their IPv6), and `ping` doesn't work.
- **No decryption.** HTTPS content stays encrypted; you see where it goes, not what is in it.
  With Private DNS in strict mode, names come only from the TLS server name. QUIC (HTTP/3)
  connections show a name only if the DNS lookup was seen.
- **Overhead.** All traffic goes through the agent, which costs some CPU and battery. Turn the capture off when you measure battery use.

## Layout

- `engine/`: the capture engine in plain Kotlin (no Android dependency): packet parsing, the
  TCP/UDP forwarding, DNS/SNI/HTTP parsing, counters, protobuf snapshots, pcap.
  `./gradlew -p engine test` runs its tests. The integration tests push real traffic through a
  Linux TUN device and run only as root on Linux; elsewhere they are skipped.
- `app/`: the Android side: the VPN service, the activity that asks for permission, the adb sockets.

After changing `proto/agent.proto`, regenerate the snapshots the Rust tests read with
`UPDATE_GOLDEN=1 ./gradlew -p engine test --tests '*SnapshotGoldenTest*'`.
