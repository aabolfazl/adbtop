use std::collections::VecDeque;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{mpsc, oneshot};

pub const MAX_LINES: usize = 10_000;
pub const BUCKETS: [&str; 5] = ["active", "working_set", "frequent", "rare", "restricted"];

#[derive(Debug)]
pub enum ConsoleEvent {
    Line { text: String, err: bool },
    Done { code: Option<i32> },
}

pub type ConsoleMsg = (u64, ConsoleEvent);

#[derive(Default)]
pub struct Console {
    pub gen: u64,
    pub title: String,
    pub lines: VecDeque<(String, bool)>,
    pub running: bool,
    pub code: Option<i32>,
    pub started: Option<Instant>,
    pub took: Option<Duration>,
    pub scroll: usize,
    pub cancel: Option<oneshot::Sender<()>>,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    pub input: String,
}

impl Console {
    pub fn next_gen(&mut self) -> u64 {
        self.stop();
        self.gen += 1;
        self.gen
    }

    pub fn start(&mut self, title: String, cancel: oneshot::Sender<()>) {
        self.title = title;
        self.lines.clear();
        self.running = true;
        self.code = None;
        self.started = Some(Instant::now());
        self.took = None;
        self.scroll = 0;
        self.cancel = Some(cancel);
    }

    pub fn on_event(&mut self, (gen, event): ConsoleMsg) {
        if gen != self.gen {
            return;
        }
        match event {
            ConsoleEvent::Line { text, err } => {
                if self.lines.len() == MAX_LINES {
                    self.lines.pop_front();
                }
                self.lines.push_back((text, err));
                if self.scroll > 0 {
                    self.scroll = (self.scroll + 1).min(self.lines.len());
                }
            }
            ConsoleEvent::Done { code } => {
                self.running = false;
                self.code = code;
                self.took = self.started.map(|s| s.elapsed());
                self.cancel = None;
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }

    pub fn remember(&mut self, command: &str) {
        if self.history.last().map(String::as_str) != Some(command) {
            self.history.push(command.to_string());
        }
        self.history_pos = None;
    }

    pub fn history_step(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let n = self.history.len();
        let pos = match (self.history_pos, back) {
            (None, true) => Some(n - 1),
            (None, false) => None,
            (Some(0), true) => Some(0),
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            (Some(_), false) => None,
        };
        self.history_pos = pos;
        self.input = pos.map(|i| self.history[i].clone()).unwrap_or_default();
    }
}

pub fn split_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    in_word = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    args.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unclosed quote".into());
    }
    if in_word {
        args.push(cur);
    }
    Ok(args)
}

pub fn console_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = split_args(input.trim())?;
    if args.first().map(String::as_str) == Some("adb") {
        args.remove(0);
    }
    if args.is_empty() {
        return Err("type an adb command, e.g. shell dumpsys battery".into());
    }
    Ok(args)
}

fn picks_device(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some(
            "-s" | "-d"
                | "-e"
                | "-t"
                | "devices"
                | "connect"
                | "disconnect"
                | "pair"
                | "kill-server"
                | "start-server"
                | "version"
                | "help"
                | "mdns"
        )
    )
}

fn clean(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        match c {
            '\r' => {}
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

async fn forward<R: AsyncRead + Unpin>(
    reader: R,
    tx: mpsc::Sender<ConsoleMsg>,
    gen: u64,
    err: bool,
) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = clean(&String::from_utf8_lossy(&buf));
                if tx
                    .send((gen, ConsoleEvent::Line { text, err }))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

pub fn spawn_adb(
    serial: Option<String>,
    args: Vec<String>,
    tx: mpsc::Sender<ConsoleMsg>,
    gen: u64,
) -> oneshot::Sender<()> {
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let serial = if picks_device(&args) { None } else { serial };
        let mut cmd = crate::adb::command(serial.as_deref());
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send((
                        gen,
                        ConsoleEvent::Line {
                            text: format!("cannot run adb: {e}"),
                            err: true,
                        },
                    ))
                    .await;
                let _ = tx.send((gen, ConsoleEvent::Done { code: None })).await;
                return;
            }
        };
        let out = child
            .stdout
            .take()
            .map(|o| tokio::spawn(forward(o, tx.clone(), gen, false)));
        let err = child
            .stderr
            .take()
            .map(|e| tokio::spawn(forward(e, tx.clone(), gen, true)));
        let code = tokio::select! {
            status = child.wait() => status.ok().and_then(|s| s.code()),
            _ = cancel_rx => {
                let _ = child.kill().await;
                let _ = tx.send((gen, ConsoleEvent::Line { text: "stopped".into(), err: true })).await;
                None
            }
        };
        for task in [out, err].into_iter().flatten() {
            let _ = tokio::time::timeout(Duration::from_millis(500), task).await;
        }
        let _ = tx.send((gen, ConsoleEvent::Done { code })).await;
    });
    cancel_tx
}

#[derive(Debug, Default, Clone)]
pub struct Tweaks {
    pub doze: bool,
    pub unplugged: bool,
    pub bucket_original: Option<(String, String)>,
    pub bucket_set: Option<String>,
}

impl Tweaks {
    pub fn any(&self) -> bool {
        self.doze || self.unplugged || self.bucket_original.is_some()
    }

    pub fn restore_script(&self) -> Option<String> {
        let mut steps = Vec::new();
        if self.doze {
            steps.push("dumpsys deviceidle unforce".to_string());
        }
        if self.doze || self.unplugged {
            steps.push("dumpsys battery reset".to_string());
        }
        if let Some((pkg, bucket)) = &self.bucket_original {
            steps.push(format!("am set-standby-bucket {pkg} {bucket}"));
        }
        (!steps.is_empty()).then(|| steps.join("; "))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Launch,
    Restart,
    ForceStop,
    Doze,
    Bucket,
    Unplug,
    ResetFrames,
    InstallAgent,
    Console,
}

pub struct ActionItem {
    pub key: char,
    pub group: &'static str,
    pub label: String,
    pub detail: &'static str,
    pub act: Act,
    pub needs_app: bool,
}

pub fn next_bucket(current: Option<&str>) -> &'static str {
    let cur = current.map(|c| c.to_lowercase());
    let i = cur
        .as_deref()
        .and_then(|c| BUCKETS.iter().position(|b| *b == c))
        .unwrap_or(0);
    BUCKETS[(i + 1) % BUCKETS.len()]
}

pub fn menu(tweaks: &Tweaks, bucket_now: Option<&str>) -> Vec<ActionItem> {
    let item = |key, group, label: String, detail, act, needs_app| ActionItem {
        key,
        group,
        label,
        detail,
        act,
        needs_app,
    };
    vec![
        item(
            'l',
            "APP",
            "Launch".into(),
            "am start -W (shows the start time)",
            Act::Launch,
            true,
        ),
        item(
            'R',
            "APP",
            "Cold restart".into(),
            "force-stop, start, measure; adds a marker",
            Act::Restart,
            true,
        ),
        item(
            'k',
            "APP",
            "Force stop".into(),
            "am force-stop",
            Act::ForceStop,
            true,
        ),
        item(
            'r',
            "FRAMES",
            "Reset frame stats".into(),
            "dumpsys gfxinfo <app> reset",
            Act::ResetFrames,
            true,
        ),
        item(
            'z',
            "POWER",
            if tweaks.doze {
                "Leave forced Doze".into()
            } else {
                "Force Doze".into()
            },
            "dumpsys deviceidle force-idle / unforce",
            Act::Doze,
            false,
        ),
        item(
            'b',
            "POWER",
            format!(
                "Standby bucket → {}",
                next_bucket(tweaks.bucket_set.as_deref().or(bucket_now))
            ),
            "am set-standby-bucket; restored on quit",
            Act::Bucket,
            true,
        ),
        item(
            'u',
            "POWER",
            if tweaks.unplugged {
                "Restore real power state".into()
            } else {
                "Fake unplug".into()
            },
            "dumpsys battery unplug / reset",
            Act::Unplug,
            false,
        ),
        item(
            'i',
            "CAPTURE",
            "Install or update the capture agent".into(),
            if crate::agent::bundled_apk().is_some() {
                "adb install -r (the agent bundled with adbtop)"
            } else {
                "not bundled in this build: adbtop agent install --apk"
            },
            Act::InstallAgent,
            false,
        ),
        item(
            ':',
            "ADB",
            "Run an adb command".into(),
            "output streams here; esc stops",
            Act::Console,
            false,
        ),
    ]
}

pub fn launch_script(pkg: &str, restart: bool) -> String {
    let flags = if restart { "-W -S" } else { "-W" };
    format!("C=$(cmd package resolve-activity --brief -c android.intent.category.LAUNCHER {pkg} | tail -n 1); echo \"am start {flags} -n $C\"; am start {flags} -n \"$C\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_a_shell() {
        assert_eq!(
            split_args("shell dumpsys battery").unwrap(),
            ["shell", "dumpsys", "battery"]
        );
        assert_eq!(
            split_args("shell 'echo a b' \"c d\" e\\ f").unwrap(),
            ["shell", "echo a b", "c d", "e f"]
        );
        assert_eq!(split_args("  ").unwrap(), Vec::<String>::new());
        assert!(split_args("shell 'oops").is_err());
        assert_eq!(console_args("adb logcat -d").unwrap(), ["logcat", "-d"]);
        assert!(console_args("adb").is_err());
    }

    #[test]
    fn cleans_terminal_codes() {
        assert_eq!(clean("\u{1b}[31mred\u{1b}[0m\r\n"), "red");
        assert_eq!(clean("a\tb"), "a    b");
    }

    #[test]
    fn device_choice_respects_user_flags() {
        assert!(picks_device(&["-s".into(), "X".into(), "shell".into()]));
        assert!(picks_device(&["devices".into()]));
        assert!(!picks_device(&["shell".into(), "ls".into()]));
    }

    #[test]
    fn tweaks_restore_everything_they_changed() {
        let t = Tweaks {
            doze: true,
            unplugged: false,
            bucket_original: Some(("com.example.app".into(), "active".into())),
            bucket_set: Some("rare".into()),
        };
        assert_eq!(t.restore_script().unwrap(), "dumpsys deviceidle unforce; dumpsys battery reset; am set-standby-bucket com.example.app active");
        assert!(Tweaks::default().restore_script().is_none());
        assert_eq!(next_bucket(Some("ACTIVE")), "working_set");
        assert_eq!(next_bucket(Some("restricted")), "active");
        assert_eq!(next_bucket(None), "working_set");
    }

    #[test]
    fn console_history_walks_back_and_forward() {
        let mut c = Console::default();
        c.remember("shell ls");
        c.remember("logcat -d");
        c.history_step(true);
        assert_eq!(c.input, "logcat -d");
        c.history_step(true);
        assert_eq!(c.input, "shell ls");
        c.history_step(false);
        assert_eq!(c.input, "logcat -d");
        c.history_step(false);
        assert_eq!(c.input, "");
    }
}
