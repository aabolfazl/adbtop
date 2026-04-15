use anyhow::{anyhow, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

pub struct ShellRequest {
    pub command: String,
    pub response_tx: oneshot::Sender<Result<String>>,
}

pub struct PersistentShell {
    request_tx: mpsc::Sender<ShellRequest>,
}

impl PersistentShell {
    pub fn spawn(device_serial: Option<String>) -> Result<Self> {
        info!("PersistentShell::spawn device={:?}", device_serial);
        let (request_tx, request_rx) = mpsc::channel::<ShellRequest>(32);

        tokio::spawn(shell_task(device_serial, request_rx));

        Ok(Self { request_tx })
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
    loop {
        let child = spawn_shell(&device_serial).await;

        match child {
            Ok(mut child) => {
                info!("Shell spawned successfully");
                if let Err(e) = run_shell_loop(&mut child, &mut request_rx).await {
                    warn!("Shell error: {}, restarting...", e);
                }
                let _ = child.kill().await;
            }
            Err(e) => {
                error!("Failed to spawn shell: {}, retrying...", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

async fn spawn_shell(device_serial: &Option<String>) -> Result<Child> {
    let mut cmd = Command::new("adb");

    if let Some(ref serial) = device_serial {
        cmd.arg("-s").arg(serial);
    }

    let child = cmd
        .arg("shell")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(child)
}

async fn run_shell_loop(
    child: &mut Child,
    request_rx: &mut mpsc::Receiver<ShellRequest>,
) -> Result<()> {
    let stdin = child.stdin.take().ok_or(anyhow!("No stdin"))?;
    let stdout = child.stdout.take().ok_or(anyhow!("No stdout"))?;

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    const END_MARKER: &str = "___END_CMD_12345___";

    while let Some(request) = request_rx.recv().await {
        let cmd_with_marker = format!(
            "{}\necho '{}'\n",
            request.command.trim(),
            END_MARKER
        );

        if let Err(e) = stdin.write_all(cmd_with_marker.as_bytes()).await {
            let _ = request.response_tx.send(Err(anyhow!("Write error: {}", e)));
            return Err(anyhow!("Write error"));
        }

        if let Err(e) = stdin.flush().await {
            let _ = request.response_tx.send(Err(anyhow!("Flush error: {}", e)));
            return Err(anyhow!("Flush error"));
        }

        let mut output = String::new();
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    let _ = request.response_tx.send(Err(anyhow!("Shell closed")));
                    return Err(anyhow!("Shell EOF"));
                }
                Ok(_) => {
                    if line.trim() == END_MARKER {
                        break;
                    }
                    output.push_str(&line);
                }
                Err(e) => {
                    let _ = request.response_tx.send(Err(anyhow!("Read error: {}", e)));
                    return Err(anyhow!("Read error"));
                }
            }
        }

        let _ = request.response_tx.send(Ok(output));
    }

    Ok(())
}
