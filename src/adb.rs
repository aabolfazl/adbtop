use anyhow::{anyhow, Context, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::model::Device;

const END_MARKER: &str = "___ADBTOP_END_7f3a___";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const RESTART_DELAY: Duration = Duration::from_secs(1);

pub fn command(serial: Option<&str>) -> Command {
    let mut cmd = Command::new("adb");
    if let Some(serial) = serial {
        cmd.arg("-s").arg(serial);
    }
    cmd.kill_on_drop(true);
    cmd
}

pub async fn devices() -> Result<Vec<Device>> {
    let output = Command::new("adb")
        .args(["devices", "-l"])
        .stdin(Stdio::null())
        .output()
        .await
        .context("adb not found; install Android platform-tools and put adb on PATH")?;
    if !output.status.success() {
        return Err(anyhow!(
            "adb devices failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(crate::parse::parse_devices(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

pub async fn shell_once(serial: Option<&str>, script: &str) -> Result<String> {
    let output = command(serial)
        .arg("shell")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .await
        .context("running adb")?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(anyhow!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct Request {
    command: String,
    reply: oneshot::Sender<Result<String>>,
}

#[derive(Clone)]
pub struct Shell {
    tx: mpsc::Sender<Request>,
}

impl Shell {
    pub fn spawn(serial: Option<String>) -> Self {
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(shell_task(serial, rx));
        Self { tx }
    }

    pub async fn exec(&self, command: impl Into<String>) -> Result<String> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(Request {
                command: command.into(),
                reply,
            })
            .await
            .map_err(|_| anyhow!("adb shell closed"))?;
        response.await.map_err(|_| anyhow!("adb shell closed"))?
    }
}

async fn shell_task(serial: Option<String>, mut rx: mpsc::Receiver<Request>) {
    while let Some(first) = rx.recv().await {
        let (failed, message) = match spawn(&serial) {
            Ok(mut child) => {
                let stderr = drain_stderr(&mut child);
                let result = serve(&mut child, first, &mut rx).await;
                let _ = child.kill().await;
                match result {
                    Ok(()) => return,
                    Err((request, e)) => (
                        request,
                        last_line(stderr).await.unwrap_or_else(|| e.to_string()),
                    ),
                }
            }
            Err(e) => (first, format!("failed to start adb: {e}")),
        };
        let _ = failed.reply.send(Err(anyhow!("{message}")));
        while let Ok(request) = rx.try_recv() {
            let _ = request.reply.send(Err(anyhow!("{message}")));
        }
        tokio::time::sleep(RESTART_DELAY).await;
    }
}

fn spawn(serial: &Option<String>) -> Result<Child> {
    Ok(command(serial.as_deref())
        .arg("shell")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?)
}

fn drain_stderr(child: &mut Child) -> Option<JoinHandle<String>> {
    let stderr = child.stderr.take()?;
    Some(tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut last = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim();
            if !line.is_empty() {
                last = line.to_string();
            }
        }
        last
    }))
}

async fn last_line(handle: Option<JoinHandle<String>>) -> Option<String> {
    let line = tokio::time::timeout(Duration::from_millis(500), handle?)
        .await
        .ok()?
        .ok()?;
    (!line.is_empty()).then_some(line)
}

async fn serve(
    child: &mut Child,
    first: Request,
    rx: &mut mpsc::Receiver<Request>,
) -> std::result::Result<(), (Request, anyhow::Error)> {
    let (mut stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
        (Some(i), Some(o)) => (i, o),
        _ => return Err((first, anyhow!("adb shell has no stdin/stdout"))),
    };
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut request = first;
    loop {
        match tokio::time::timeout(
            COMMAND_TIMEOUT,
            run(&mut stdin, &mut reader, &mut buf, &request.command),
        )
        .await
        {
            Ok(Ok(output)) => {
                let _ = request.reply.send(Ok(output));
            }
            Ok(Err(e)) => return Err((request, e)),
            Err(_) => return Err((request, anyhow!("adb command timed out"))),
        }
        request = match rx.recv().await {
            Some(r) => r,
            None => return Ok(()),
        };
    }
}

async fn run(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    buf: &mut Vec<u8>,
    command: &str,
) -> Result<String> {
    let script = format!("{}\necho '{END_MARKER}'\n", command.trim());
    stdin.write_all(script.as_bytes()).await?;
    stdin.flush().await?;
    buf.clear();
    let marker = END_MARKER.as_bytes();
    loop {
        let start = buf.len();
        if reader.read_until(b'\n', buf).await? == 0 {
            return Err(anyhow!("adb shell exited"));
        }
        let line = trim_newline(&buf[start..]);
        if line.ends_with(marker) {
            let keep = start + line.len() - marker.len();
            buf.truncate(keep);
            return Ok(String::from_utf8_lossy(buf).into_owned());
        }
    }
}

fn trim_newline(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_trimming() {
        assert_eq!(trim_newline(b"abc\r\n"), b"abc");
        assert_eq!(trim_newline(b"\n"), b"");
    }
}
