//! Installs the bundled Claude Code skill, so one `cargo install` covers both.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The skill is compiled into the binary, so it always matches the commands in this build.
const SKILL: &str = include_str!("../../../skills/knowledge-bae/SKILL.md");
const NAME: &str = "knowledge-bae";

/// `~/.claude/skills`, where Claude Code looks for personal skills.
pub fn personal_skills_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir().context("could not find your home directory")?.join(".claude").join("skills"))
}

/// Writes `<skills_dir>/knowledge-bae/SKILL.md`. Returns the file and whether it replaced an older copy.
pub fn install(skills_dir: &Path) -> Result<(PathBuf, bool)> {
    let dir = skills_dir.join(NAME);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join("SKILL.md");
    let replaced = file.exists();
    std::fs::write(&file, SKILL).with_context(|| format!("writing {}", file.display()))?;
    Ok((file, replaced))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skill_has_valid_frontmatter() {
        let rest = SKILL.strip_prefix("---\n").expect("starts with frontmatter");
        let (front, _body) = rest.split_once("\n---\n").expect("frontmatter is closed");
        assert!(front.lines().any(|l| l == "name: knowledge-bae"));
        let description = front.lines().find_map(|l| l.strip_prefix("description: ")).unwrap();
        assert!(description.len() > 100, "the description is what makes Claude pick this skill");
    }

    #[test]
    fn installs_then_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let (file, replaced) = install(dir.path()).unwrap();
        assert!(!replaced);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), SKILL);
        assert!(file.ends_with("knowledge-bae/SKILL.md"));

        std::fs::write(&file, "old").unwrap();
        let (_, replaced) = install(dir.path()).unwrap();
        assert!(replaced);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), SKILL);
    }
}
