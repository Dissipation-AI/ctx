//! `ctx install instructions for …`: ctx's block in agents' instruction files.

use ctx_cli::parse::Format;

pub const BEGIN: &str = "<!-- ctx instructions: begin -->";
pub const END: &str = "<!-- ctx instructions: end -->";

/// Where each agent reads its instructions (checked against current versions; kept here only).
pub fn file_for(agent: &str, folder: Option<&str>) -> std::path::PathBuf {
    let home = std::path::PathBuf::from(ctx_server::sys::home());
    match agent {
        "claude" => home.join(".claude/CLAUDE.md"),
        "codex" => home.join(".codex/AGENTS.md"),
        "gemini" => home.join(".gemini/GEMINI.md"),
        _ => {
            let dir = std::path::PathBuf::from(folder.unwrap_or("."));
            let agents = dir.join("AGENTS.md");
            let claude = dir.join("CLAUDE.md");
            if !agents.exists() && claude.exists() { claude } else { agents }
        }
    }
}

/// Replace (or add) ctx's block, touching nothing outside the markers.
pub fn with_block(existing: &str, block: &str) -> String {
    let wrapped = format!("{BEGIN}\n{}\n{END}", block.trim_end());
    if let (Some(b), Some(e)) = (existing.find(BEGIN), existing.find(END))
        && e > b
    {
        let mut out = String::new();
        out.push_str(&existing[..b]);
        out.push_str(&wrapped);
        out.push_str(&existing[e + END.len()..]);
        return out;
    }
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&wrapped);
    out.push('\n');
    out
}

/// Remove ctx's block and the blank line before it.
pub fn without_block(existing: &str) -> Option<String> {
    let b = existing.find(BEGIN)?;
    let e = existing.find(END)?;
    if e < b {
        return None;
    }
    let mut before = existing[..b].to_string();
    let after = &existing[e + END.len()..];
    let after = after.strip_prefix('\n').unwrap_or(after);
    if before.ends_with("\n\n") {
        before.pop();
    }
    Some(format!("{before}{after}"))
}

fn report(msg: String, format: Format) {
    match format {
        Format::Text => println!("{msg}"),
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&ctx_protocol::with_schema(serde_json::json!({ "message": msg })))
                .unwrap_or_default()
        ),
        Format::Toon => println!(
            "{}",
            ctx_protocol::toon::encode(&ctx_protocol::with_schema(serde_json::json!({ "message": msg })))
        ),
    }
}

pub fn install(agent: &str, folder: Option<&str>, format: Format) -> i32 {
    let path = file_for(agent, folder);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = with_block(&existing, &ctx_cli::help::agent_instructions());
    if let Some(d) = path.parent()
        && let Err(e) = std::fs::create_dir_all(d)
    {
        eprintln!("Couldn't create {}: {e}", d.display());
        return 1;
    }
    let verb = if existing.contains(BEGIN) { "Updated" } else { "Added" };
    if existing == updated {
        report(format!("ctx's instructions in {} are already up to date.", path.display()), format);
        return 0;
    }
    match std::fs::write(&path, updated) {
        Ok(()) => {
            report(format!("{verb} ctx's instructions in {}.", path.display()), format);
            0
        }
        Err(e) => {
            eprintln!("Couldn't write {}: {e}", path.display());
            1
        }
    }
}

pub fn remove(agent: &str, folder: Option<&str>, format: Format) -> i32 {
    let path = file_for(agent, folder);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        report(format!("{} doesn't exist, so there's nothing to remove.", path.display()), format);
        return 0;
    };
    match without_block(&existing) {
        Some(updated) => match std::fs::write(&path, updated) {
            Ok(()) => {
                report(format!("Removed ctx's instructions from {}.", path.display()), format);
                0
            }
            Err(e) => {
                eprintln!("Couldn't write {}: {e}", path.display());
                1
            }
        },
        None => {
            report(format!("{} has no ctx instructions.", path.display()), format);
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_is_added_updated_and_removed_without_touching_the_rest() {
        let original = "# My notes\n\nKeep this.\n";
        let a = with_block(original, "v1");
        assert!(a.starts_with(original));
        assert!(a.contains("v1"));
        let b = with_block(&a, "v2");
        assert!(b.contains("v2") && !b.contains("v1"));
        assert_eq!(b.matches(BEGIN).count(), 1);
        assert_eq!(without_block(&b).unwrap(), original);
        assert_eq!(with_block("", "x"), format!("{BEGIN}\nx\n{END}\n"));
    }
}
