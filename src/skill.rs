use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::SkillAction;
use crate::headless::{fail, print_json, EXIT_ERROR, EXIT_NOT_FOUND};

const SKILL: &str = include_str!("../skills/adbtop/SKILL.md");
const NAME: &str = "adbtop";

#[derive(Serialize)]
struct Target {
    agent: &'static str,
    path: String,
}

#[derive(Serialize)]
struct Installed {
    skill: &'static str,
    version: &'static str,
    installed: Vec<Target>,
}

#[derive(Serialize)]
struct Removed {
    skill: &'static str,
    removed: Vec<Target>,
}

fn home_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| crate::config::home().map(|h| h.join(fallback)))
}

fn claude_home() -> Option<PathBuf> {
    home_dir("CLAUDE_CONFIG_DIR", ".claude")
}

fn codex_home() -> Option<PathBuf> {
    home_dir("CODEX_HOME", ".codex")
}

fn write_skill(skills_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = skills_dir.join(NAME);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("SKILL.md");
    std::fs::write(&file, SKILL)?;
    Ok(file)
}

pub fn run(action: SkillAction) -> i32 {
    match action {
        SkillAction::Install {
            claude,
            codex,
            dest,
        } => install(claude, codex, dest),
        SkillAction::Uninstall => uninstall(),
    }
}

fn install(claude: bool, codex: bool, dest: Option<PathBuf>) -> i32 {
    let auto = !claude && !codex && dest.is_none();
    let mut targets: Vec<(&'static str, PathBuf)> = Vec::new();
    for (agent, wanted, home) in [
        ("claude", claude, claude_home()),
        ("codex", codex, codex_home()),
    ] {
        if let Some(home) = home {
            if wanted || (auto && home.is_dir()) {
                targets.push((agent, home.join("skills")));
            }
        }
    }
    if let Some(dir) = dest {
        targets.push(("other", dir));
    }
    if targets.is_empty() {
        return fail(
            "no_agent_found",
            "found neither Claude Code (~/.claude) nor Codex (~/.codex); pass --claude, --codex or --dest DIR",
            EXIT_NOT_FOUND,
        );
    }
    let mut installed = Vec::new();
    for (agent, dir) in targets {
        match write_skill(&dir) {
            Ok(path) => installed.push(Target {
                agent,
                path: path.display().to_string(),
            }),
            Err(e) => {
                return fail(
                    "skill_write_failed",
                    format!("{}: {e}", dir.join(NAME).display()),
                    EXIT_ERROR,
                )
            }
        }
    }
    print_json(&Installed {
        skill: NAME,
        version: env!("CARGO_PKG_VERSION"),
        installed,
    })
}

fn uninstall() -> i32 {
    let mut removed = Vec::new();
    for (agent, home) in [("claude", claude_home()), ("codex", codex_home())] {
        let Some(home) = home else { continue };
        let dir = home.join("skills").join(NAME);
        let file = dir.join("SKILL.md");
        if !file.is_file() {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&file) {
            return fail(
                "skill_remove_failed",
                format!("{}: {e}", file.display()),
                EXIT_ERROR,
            );
        }
        let _ = std::fs::remove_dir(&dir);
        removed.push(Target {
            agent,
            path: file.display().to_string(),
        });
    }
    print_json(&Removed {
        skill: NAME,
        removed,
    })
}
