//! Talks to the on-device capture agent (android-agent/) over `adb forward`.

use anyhow::{anyhow, Context, Result};
use prost::Message;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::runner::PanelUpdate;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/perfagent.rs"));
}

const CONTROL_ACTIVITY: &str = "io.github.aabolfazl.perfagent/.ControlActivity";
const STATS_SOCKET: &str = "android-perf-agent";
const PCAP_SOCKET: &str = "android-perf-agent-pcap";
const PCAP_HEADER_LEN: usize = 24;
/// Snapshots are small; anything bigger means the stream is out of sync.
const MAX_MESSAGE_LEN: usize = 16 * 1024 * 1024;

fn adb(serial: Option<&str>) -> Command {
    let mut cmd = Command::new("adb");
    if let Some(serial) = serial {
        cmd.arg("-s").arg(serial);
    }
    cmd
}

/// Starts the capture on the phone. The first time, Android asks there for VPN permission.
pub async fn start_capture(serial: Option<&str>, app: Option<&str>) -> Result<()> {
    let mut cmd = adb(serial);
    cmd.args(["shell", "am", "start", "-n", CONTROL_ACTIVITY, "--es", "cmd", "start"]);
    if let Some(app) = app {
        cmd.args(["--es", "app", app]);
    }
    run_am(cmd).await
}

pub async fn stop_capture(serial: Option<&str>) -> Result<()> {
    let mut cmd = adb(serial);
    cmd.args(["shell", "am", "start", "-n", CONTROL_ACTIVITY, "--es", "cmd", "stop"]);
    run_am(cmd).await
}

async fn run_am(mut cmd: Command) -> Result<()> {
    let output = cmd.output().await.context("running adb")?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if !output.status.success() || text.contains("Error") {
        if text.contains("does not exist") {
            return Err(anyhow!("capture agent is not installed (see android-agent/README.md)"));
        }
        let line = text.lines().find(|l| l.contains("Error")).unwrap_or(text.trim());
        return Err(anyhow!("{}", line.trim()));
    }
    Ok(())
}

/// An `adb forward` from a free local port to a socket on the phone, removed on drop.
struct Forward {
    serial: Option<String>,
    port: u16,
}

impl Forward {
    async fn create(serial: Option<&str>, socket: &str) -> Result<Self> {
        let output = adb(serial)
            .args(["forward", "tcp:0", &format!("localabstract:{socket}")])
            .output()
            .await
            .context("running adb forward")?;
        let text = String::from_utf8_lossy(&output.stdout);
        let port = text
            .trim()
            .parse::<u16>()
            .map_err(|_| anyhow!("adb forward failed: {}", String::from_utf8_lossy(&output.stderr).trim()))?;
        Ok(Self { serial: serial.map(str::to_string), port })
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        let mut cmd = std::process::Command::new("adb");
        if let Some(ref serial) = self.serial {
            cmd.arg("-s").arg(serial);
        }
        let _ = cmd.args(["forward", "--remove", &format!("tcp:{}", self.port)]).output();
    }
}

/// Reads one varint-length-prefixed message; `None` at a clean end of stream.
pub async fn read_delimited<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    let mut len: u64 = 0;
    for i in 0..10 {
        let byte = match reader.read_u8().await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && i == 0 => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        len |= u64::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            let len = len as usize;
            if len > MAX_MESSAGE_LEN {
                return Err(anyhow!("message of {len} bytes is too large"));
            }
            let mut buf = vec![0; len];
            reader.read_exact(&mut buf).await?;
            return Ok(Some(buf));
        }
    }
    Err(anyhow!("invalid message length"))
}

/// Keeps a connection to the agent and forwards its snapshots. When the agent isn't running,
/// adb accepts the local connection and closes it right away, so this just retries quietly.
pub async fn snapshot_runner(tx: mpsc::UnboundedSender<PanelUpdate>, serial: Option<String>) {
    let mut forward: Option<Forward> = None;
    let mut connected = false;

    loop {
        if forward.is_none() {
            match Forward::create(serial.as_deref(), STATS_SOCKET).await {
                Ok(f) => forward = Some(f),
                Err(e) => {
                    warn!("agent forward: {e}");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            }
        }
        let port = forward.as_ref().map(|f| f.port).unwrap_or_default();

        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)).await {
            loop {
                match read_delimited(&mut stream).await {
                    Ok(Some(bytes)) => match proto::Snapshot::decode(bytes.as_slice()) {
                        Ok(snapshot) => {
                            if !connected {
                                info!("connected to the capture agent");
                                connected = true;
                            }
                            if tx.send(PanelUpdate::Capture(Box::new(snapshot))).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            warn!("bad snapshot from agent: {e}");
                            break;
                        }
                    },
                    Ok(None) => break,
                    Err(e) => {
                        warn!("agent connection: {e}");
                        break;
                    }
                }
            }
        }

        if connected {
            connected = false;
            let _ = tx.send(PanelUpdate::CaptureStopped);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Saves the agent's live pcap stream to `path`, reconnecting (without repeating the file
/// header) whenever the capture restarts.
pub async fn pcap_runner(tx: mpsc::UnboundedSender<PanelUpdate>, serial: Option<String>, path: PathBuf) {
    let mut file = match tokio::fs::File::create(&path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = tx.send(PanelUpdate::Error { panel: "pcap", message: format!("{}: {e}", path.display()) });
            return;
        }
    };
    let mut header_written = false;
    let mut forward: Option<Forward> = None;

    loop {
        if forward.is_none() {
            forward = Forward::create(serial.as_deref(), PCAP_SOCKET).await.ok();
        }
        if let Some(port) = forward.as_ref().map(|f| f.port) {
            if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)).await {
                let mut header = [0u8; PCAP_HEADER_LEN];
                if stream.read_exact(&mut header).await.is_ok() {
                    if !header_written {
                        if file.write_all(&header).await.is_err() {
                            return;
                        }
                        header_written = true;
                    }
                    if let Err(e) = tokio::io::copy(&mut stream, &mut file).await {
                        warn!("pcap stream: {e}");
                    }
                    let _ = file.flush().await;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn sample() -> proto::Snapshot {
        proto::Snapshot {
            agent_version: "0.1.0".into(),
            time_ms: 2000,
            started_ms: 1000,
            apps: vec![proto::AppTraffic { uid: 10123, name: "com.example.app".into(), rx_bytes: 300, tx_bytes: 40, open_connections: 1 }],
            connections: vec![proto::Connection {
                id: 1,
                protocol: proto::Protocol::Tcp as i32,
                uid: 10123,
                remote_ip: vec![142, 250, 1, 1],
                remote_port: 443,
                host: "www.example.com".into(),
                host_source: proto::HostSource::TlsSni as i32,
                state: proto::ConnectionState::Open as i32,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn reads_length_delimited_snapshots() {
        let mut bytes = Vec::new();
        sample().encode_length_delimited(&mut bytes).unwrap();
        let mut second = sample();
        second.time_ms = 3000;
        second.encode_length_delimited(&mut bytes).unwrap();

        let mut reader = bytes.as_slice();
        let first = read_delimited(&mut reader).await.unwrap().unwrap();
        assert_eq!(proto::Snapshot::decode(first.as_slice()).unwrap(), sample());
        let next = read_delimited(&mut reader).await.unwrap().unwrap();
        assert_eq!(proto::Snapshot::decode(next.as_slice()).unwrap().time_ms, 3000);
        assert!(read_delimited(&mut reader).await.unwrap().is_none());
    }

    /// Snapshots written by the Kotlin agent code (android-agent/engine SnapshotGoldenTest).
    pub async fn golden_snapshots() -> Vec<proto::Snapshot> {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/proto/testdata/snapshots.bin")).unwrap();
        let mut reader = bytes.as_slice();
        let mut snapshots = Vec::new();
        while let Some(message) = read_delimited(&mut reader).await.unwrap() {
            snapshots.push(proto::Snapshot::decode(message.as_slice()).unwrap());
        }
        snapshots
    }

    #[tokio::test]
    async fn decodes_what_the_kotlin_agent_writes() {
        let snapshots = golden_snapshots().await;
        assert_eq!(snapshots.len(), 5);
        let last = &snapshots[4];
        assert_eq!(last.agent_version, "golden");
        assert_eq!(last.time_ms - last.started_ms, 5000);

        let app = last.apps.iter().find(|a| a.uid == 10123).unwrap();
        assert_eq!(app.name, "com.example.app");
        assert_eq!(app.rx_bytes, 75_000);

        let web = last.connections.iter().find(|c| c.host == "www.example.com").unwrap();
        assert_eq!(web.remote_ip, vec![93, 184, 216, 34]);
        assert_eq!(web.remote_port, 443);
        assert_eq!(web.host_source, proto::HostSource::TlsSni as i32);
        assert_eq!(web.state, proto::ConnectionState::Open as i32);
        let api = last.connections.iter().find(|c| c.host == "api.example.com").unwrap();
        assert_eq!(api.state, proto::ConnectionState::Closed as i32);

        assert_eq!(last.dns[0].name, "www.example.com");
        assert_eq!(last.dns[0].addresses, vec![vec![93, 184, 216, 34]]);
        assert_eq!(last.stats.unwrap().open_tcp, 1);
    }

    #[tokio::test]
    async fn missing_agent_is_reported_clearly() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'Error type 3'; echo 'Error: Activity class {io.github.aabolfazl.perfagent/.ControlActivity} does not exist.'"]);
        let error = run_am(cmd).await.unwrap_err().to_string();
        assert!(error.contains("not installed"), "{error}");

        let mut ok = Command::new("sh");
        ok.args(["-c", "echo 'Starting: Intent { cmp=io.github.aabolfazl.perfagent/.ControlActivity }'"]);
        assert!(run_am(ok).await.is_ok());
    }

    #[tokio::test]
    async fn truncated_message_is_an_error() {
        let mut bytes = Vec::new();
        sample().encode_length_delimited(&mut bytes).unwrap();
        bytes.truncate(bytes.len() - 3);
        assert!(read_delimited(&mut bytes.as_slice()).await.is_err());
    }
}
