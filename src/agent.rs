use anyhow::{anyhow, Context, Result};
use prost::Message;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::collect::Update;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/perfagent.rs"));
}

const CONTROL_ACTIVITY: &str = "io.github.aabolfazl.perfagent/.ControlActivity";
const STATS_SOCKET: &str = "android-perf-agent";
const PCAP_SOCKET: &str = "android-perf-agent-pcap";
const PCAP_HEADER_LEN: usize = 24;
const PCAP_RECORD_HEADER_LEN: usize = 16;
const MAX_PCAP_RECORD_LEN: usize = 256 * 1024;
const MAX_MESSAGE_LEN: usize = 16 * 1024 * 1024;

pub const NOT_INSTALLED: &str = "capture agent is not installed (see android-agent/README.md)";

pub const IDLE_STOP_SECS: u32 = 30;

pub async fn start_capture(serial: Option<&str>, app: Option<&str>) -> Result<()> {
    let mut cmd = crate::adb::command(serial);
    let idle = IDLE_STOP_SECS.to_string();
    cmd.args([
        "shell",
        "am",
        "start",
        "-n",
        CONTROL_ACTIVITY,
        "--es",
        "cmd",
        "start",
        "--ei",
        "idle_stop",
        &idle,
    ]);
    if let Some(app) = app {
        cmd.args(["--es", "app", app]);
    }
    run_am(cmd).await
}

pub fn stop_script() -> String {
    format!("am start -n {CONTROL_ACTIVITY} --es cmd stop")
}

pub async fn stop_capture(serial: Option<&str>) -> Result<()> {
    let mut cmd = crate::adb::command(serial);
    cmd.args([
        "shell",
        "am",
        "start",
        "-n",
        CONTROL_ACTIVITY,
        "--es",
        "cmd",
        "stop",
    ]);
    run_am(cmd).await
}

async fn run_am(mut cmd: Command) -> Result<()> {
    let output = cmd.output().await.context("running adb")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() || text.contains("Error") {
        if text.contains("does not exist") {
            return Err(anyhow!(NOT_INSTALLED));
        }
        let line = text
            .lines()
            .find(|l| l.contains("Error"))
            .unwrap_or(text.trim());
        return Err(anyhow!("{}", line.trim()));
    }
    Ok(())
}

struct Forward {
    serial: Option<String>,
    port: u16,
    owned: bool,
}

fn existing_forward(list: &str, serial: Option<&str>, socket: &str) -> Option<u16> {
    let target = format!("localabstract:{socket}");
    list.lines().find_map(|line| {
        let mut parts = line.split_ascii_whitespace();
        let (s, local, remote) = (parts.next()?, parts.next()?, parts.next()?);
        if remote != target || serial.is_some_and(|want| want != s) {
            return None;
        }
        local.strip_prefix("tcp:")?.parse().ok()
    })
}

impl Forward {
    async fn create(serial: Option<&str>, socket: &str) -> Result<Self> {
        if let Ok(list) = crate::adb::command(serial)
            .args(["forward", "--list"])
            .output()
            .await
        {
            if let Some(port) =
                existing_forward(&String::from_utf8_lossy(&list.stdout), serial, socket)
            {
                return Ok(Self {
                    serial: serial.map(str::to_string),
                    port,
                    owned: false,
                });
            }
        }
        let output = crate::adb::command(serial)
            .args(["forward", "tcp:0", &format!("localabstract:{socket}")])
            .output()
            .await
            .context("running adb forward")?;
        let port = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u16>()
            .map_err(|_| {
                anyhow!(
                    "adb forward failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )
            })?;
        Ok(Self {
            serial: serial.map(str::to_string),
            port,
            owned: true,
        })
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let mut cmd = std::process::Command::new("adb");
        if let Some(ref serial) = self.serial {
            cmd.arg("-s").arg(serial);
        }
        let _ = cmd
            .args(["forward", "--remove", &format!("tcp:{}", self.port)])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

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

pub async fn snapshot_runner(tx: mpsc::Sender<Update>, serial: Option<String>) {
    let mut forward: Option<Forward> = None;
    let mut connected = false;
    loop {
        if forward.is_none() {
            match Forward::create(serial.as_deref(), STATS_SOCKET).await {
                Ok(f) => forward = Some(f),
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            }
        }
        let port = forward.as_ref().map(|f| f.port).unwrap_or_default();
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(mut stream) => {
                let _ = stream.set_nodelay(true);
                while let Ok(Some(bytes)) = read_delimited(&mut stream).await {
                    let Ok(snapshot) = proto::Snapshot::decode(bytes.as_slice()) else {
                        break;
                    };
                    connected = true;
                    if tx.send(Update::Capture(Box::new(snapshot))).await.is_err() {
                        return;
                    }
                }
            }
            Err(_) => forward = None,
        }
        if connected {
            connected = false;
            if tx.send(Update::CaptureStopped).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn copy_records<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    little_endian: bool,
) -> std::io::Result<()> {
    let mut record = Vec::with_capacity(PCAP_RECORD_HEADER_LEN + 65_536);
    loop {
        record.resize(PCAP_RECORD_HEADER_LEN, 0);
        reader.read_exact(&mut record).await?;
        let incl = [record[8], record[9], record[10], record[11]];
        let len = if little_endian {
            u32::from_le_bytes(incl)
        } else {
            u32::from_be_bytes(incl)
        } as usize;
        if len > MAX_PCAP_RECORD_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "pcap record too large",
            ));
        }
        record.resize(PCAP_RECORD_HEADER_LEN + len, 0);
        reader
            .read_exact(&mut record[PCAP_RECORD_HEADER_LEN..])
            .await?;
        writer.write_all(&record).await?;
    }
}

pub async fn pcap_runner(
    tx: mpsc::Sender<Update>,
    serial: Option<String>,
    path: PathBuf,
    append: bool,
) {
    let opened = if append {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
    } else {
        tokio::fs::File::create(&path).await
    };
    let mut file = match opened {
        Ok(f) => f,
        Err(e) => {
            let _ = tx
                .send(Update::CaptureError(format!("{}: {e}", path.display())))
                .await;
            return;
        }
    };
    let mut header_written = file
        .metadata()
        .await
        .is_ok_and(|m| m.len() >= PCAP_HEADER_LEN as u64);
    let mut forward: Option<Forward> = None;
    loop {
        if forward.is_none() {
            forward = Forward::create(serial.as_deref(), PCAP_SOCKET).await.ok();
        }
        if let Some(port) = forward.as_ref().map(|f| f.port) {
            match TcpStream::connect(("127.0.0.1", port)).await {
                Ok(mut stream) => {
                    let mut header = [0u8; PCAP_HEADER_LEN];
                    if stream.read_exact(&mut header).await.is_ok() {
                        if !header_written {
                            if file.write_all(&header).await.is_err() {
                                return;
                            }
                            header_written = true;
                        }
                        let little_endian = header[..4] == [0xd4, 0xc3, 0xb2, 0xa1]
                            || header[..4] == [0x4d, 0x3c, 0xb2, 0xa1];
                        let _ = copy_records(&mut stream, &mut file, little_endian).await;
                        let _ = file.flush().await;
                    }
                }
                Err(_) => forward = None,
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
            apps: vec![proto::AppTraffic {
                uid: 10123,
                name: "com.example.app".into(),
                rx_bytes: 300,
                tx_bytes: 40,
                open_connections: 1,
            }],
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
        assert_eq!(
            proto::Snapshot::decode(next.as_slice()).unwrap().time_ms,
            3000
        );
        assert!(read_delimited(&mut reader).await.unwrap().is_none());
    }

    pub async fn golden_snapshots() -> Vec<proto::Snapshot> {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/proto/testdata/snapshots.bin"
        ))
        .unwrap();
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
        let web = last
            .connections
            .iter()
            .find(|c| c.host == "www.example.com")
            .unwrap();
        assert_eq!(web.remote_ip, vec![93, 184, 216, 34]);
        assert_eq!(web.remote_port, 443);
        assert_eq!(web.host_source, proto::HostSource::TlsSni as i32);
        assert_eq!(web.state, proto::ConnectionState::Open as i32);
        let api = last
            .connections
            .iter()
            .find(|c| c.host == "api.example.com")
            .unwrap();
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
        ok.args([
            "-c",
            "echo 'Starting: Intent { cmp=io.github.aabolfazl.perfagent/.ControlActivity }'",
        ]);
        assert!(run_am(ok).await.is_ok());
    }

    #[tokio::test]
    async fn pcap_copy_keeps_whole_records_only() {
        let mut stream = Vec::new();
        for len in [3u32, 5] {
            stream.extend_from_slice(&[0; 8]);
            stream.extend_from_slice(&len.to_le_bytes());
            stream.extend_from_slice(&len.to_le_bytes());
            stream.extend(std::iter::repeat(7u8).take(len as usize));
        }
        stream.extend_from_slice(&[0; 10]);
        let mut out = Vec::new();
        assert!(copy_records(&mut stream.as_slice(), &mut out, true)
            .await
            .is_err());
        assert_eq!(out.len(), 16 + 3 + 16 + 5);
    }

    #[test]
    fn reuses_a_forward_left_by_an_earlier_run() {
        let list = "RFCX11HALEL tcp:58638 localabstract:android-perf-agent\nRFCX11HALEL tcp:58999 localabstract:android-perf-agent-pcap\nOTHER tcp:1 localabstract:android-perf-agent\n";
        assert_eq!(
            existing_forward(list, Some("RFCX11HALEL"), STATS_SOCKET),
            Some(58638)
        );
        assert_eq!(
            existing_forward(list, Some("RFCX11HALEL"), PCAP_SOCKET),
            Some(58999)
        );
        assert_eq!(existing_forward(list, Some("NOPE"), STATS_SOCKET), None);
    }

    #[tokio::test]
    async fn truncated_message_is_an_error() {
        let mut bytes = Vec::new();
        sample().encode_length_delimited(&mut bytes).unwrap();
        bytes.truncate(bytes.len() - 3);
        assert!(read_delimited(&mut bytes.as_slice()).await.is_err());
    }
}
