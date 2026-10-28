use anyhow::{anyhow, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

const END_MARKER: &str = "___END_CMD_12345___";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const RESTART_DELAY: Duration = Duration::from_secs(1);

pub struct ShellRequest {
    pub command: String,
    pub response_tx: oneshot::Sender<Result<String>>,
}

pub struct PersistentShell {
    request_tx: mpsc::Sender<ShellRequest>,
}

impl PersistentShell {
    /// The adb process lives as long as this handle; dropping it stops the shell.
    pub fn spawn(device_serial: Option<String>) -> Self {
        info!("PersistentShell::spawn device={:?}", device_serial);
        let (request_tx, request_rx) = mpsc::channel::<ShellRequest>(32);

        tokio::spawn(shell_task(device_serial, request_rx));

        Self { request_tx }
    }

    pub async fn exec(&self, command: &str) -> Result<String> {
        let (response_tx, response_rx) = oneshot::channel();

        self.request_tx
            .send(ShellRequest {
                command: command.to_string(),
                response_tx,
            })
            .await
            .map_err(|_| anyhow!("Shell task closed"))?;

        response_rx.await.map_err(|_| anyhow!("Response channel closed"))?
    }
}

async fn shell_task(device_serial: Option<String>, mut request_rx: mpsc::Receiver<ShellRequest>) {
    // Only (re)start adb when there is work, so a missing device does not spawn processes in a loop.
    while let Some(first) = request_rx.recv().await {
        let (failed, message) = match spawn_shell(&device_serial) {
            Ok(mut child) => {
                info!("Shell spawned successfully");
                let stderr = drain_stderr(&mut child);
                let result = run_shell_loop(&mut child, first, &mut request_rx).await;
                let _ = child.kill().await;
                match result {
                    Ok(()) => return,
                    Err((request, e)) => {
                        let adb_error = last_stderr_line(stderr).await;
                        (request, adb_error.unwrap_or_else(|| e.to_string()))
                    }
                }
            }
            Err(e) => (first, format!("Failed to start adb: {}", e)),
        };

        warn!("Shell error: {}, restarting...", message);
        let _ = failed.response_tx.send(Err(anyhow!("{}", message)));
        // Commands queued for the dead shell would fail the same way.
        while let Ok(request) = request_rx.try_recv() {
            let _ = request.response_tx.send(Err(anyhow!("{}", message)));
        }

        tokio::time::sleep(RESTART_DELAY).await;
    }
}

fn spawn_shell(device_serial: &Option<String>) -> Result<Child> {
    let mut cmd = Command::new("adb");

    if let Some(ref serial) = device_serial {
        cmd.arg("-s").arg(serial);
    }

    let child = cmd
        .arg("shell")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    Ok(child)
}

/// Keeps reading adb's stderr so the pipe never fills up, and returns its last line once adb exits.
fn drain_stderr(child: &mut Child) -> Option<JoinHandle<String>> {
    let stderr = child.stderr.take()?;
    Some(tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut last = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                last = line.trim().to_string();
            }
        }
        last
    }))
}

async fn last_stderr_line(handle: Option<JoinHandle<String>>) -> Option<String> {
    let handle = handle?;
    let line = tokio::time::timeout(Duration::from_millis(500), handle).await.ok()?.ok()?;
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Runs requests until the request channel closes (`Ok`) or the shell breaks,
/// in which case the request that was in flight is handed back with the error.
async fn run_shell_loop(
    child: &mut Child,
    first: ShellRequest,
    request_rx: &mut mpsc::Receiver<ShellRequest>,
) -> std::result::Result<(), (ShellRequest, anyhow::Error)> {
    let (mut stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => (stdin, stdout),
        _ => return Err((first, anyhow!("adb shell has no stdin/stdout"))),
    };
    let mut reader = BufReader::new(stdout);

    let mut request = first;
    loop {
        let result = tokio::time::timeout(
            COMMAND_TIMEOUT,
            run_command(&mut stdin, &mut reader, &request.command),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let _ = request.response_tx.send(Ok(output));
            }
            Ok(Err(e)) => return Err((request, e)),
            // The reply stream is out of sync now, so the shell has to be restarted.
            Err(_) => return Err((request, anyhow!("adb command timed out"))),
        }

        request = match request_rx.recv().await {
            Some(request) => request,
            None => return Ok(()),
        };
    }
}

async fn run_command(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    command: &str,
) -> Result<String> {
    let cmd_with_marker = format!("{}\necho '{}'\n", command.trim(), END_MARKER);
    stdin.write_all(cmd_with_marker.as_bytes()).await?;
    stdin.flush().await?;

    let mut output = String::new();
    let mut line = Vec::new();

    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Err(anyhow!("adb shell exited"));
        }
        // Thread and process names are not guaranteed to be valid UTF-8.
        let text = String::from_utf8_lossy(&line);
        if let Some(rest) = strip_end_marker(&text) {
            output.push_str(rest);
            return Ok(output);
        }
        output.push_str(&text);
    }
}

/// Output that does not end in a newline puts the marker on the same line as the last output.
fn strip_end_marker(line: &str) -> Option<&str> {
    line.trim_end().strip_suffix(END_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_marker_on_its_own_line() {
        assert_eq!(strip_end_marker("___END_CMD_12345___\n"), Some(""));
    }

    #[test]
    fn end_marker_after_output_without_newline() {
        assert_eq!(strip_end_marker("10___END_CMD_12345___\n"), Some("10"));
    }

    #[test]
    fn regular_line_is_not_a_marker() {
        assert_eq!(strip_end_marker("cpu0 1 2 3 4\n"), None);
    }
}
