//! The command line for starting a CLI agent in the current workspace.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliAgentSettings {
    pub name: String,
    pub executable: String,
    pub add_dir_arg: String,
}

impl Default for CliAgentSettings {
    fn default() -> Self {
        Self {
            name: "Claude Code".to_owned(),
            executable: "claude".to_owned(),
            add_dir_arg: "--add-dir".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliAgentCommand {
    pub agent_name: String,
    pub base_dir: PathBuf,
    pub command: String,
    pub preview: String,
}

pub fn build_cli_agent_command(
    settings: &CliAgentSettings,
    root_folders: &[PathBuf],
) -> Result<CliAgentCommand, String> {
    let Some(base_dir) = root_folders.first() else {
        return Err("No workspace folder is available.".to_owned());
    };
    let executable = settings.executable.trim();
    if executable.is_empty() {
        return Err("AI command executable is empty.".to_owned());
    }
    let add_dir_arg = settings.add_dir_arg.trim();
    if root_folders.len() > 1 && add_dir_arg.is_empty() {
        return Err("AI command add-dir option is empty.".to_owned());
    }

    let cd_line = format!("cd {}", quote_path(base_dir));
    let mut run_line = executable.to_owned();
    let mut preview = format!("{cd_line}\n{executable}");

    for root in root_folders.iter().skip(1) {
        run_line.push_str(&format!(" {add_dir_arg} {}", quote_path(root)));
        preview.push_str(&format!("\n  {add_dir_arg} {}", quote_path(root)));
    }
    let command = format!("{cd_line}\n{run_line}");

    Ok(CliAgentCommand {
        agent_name: settings.name.clone(),
        base_dir: base_dir.clone(),
        command,
        preview,
    })
}

fn quote_path(path: &Path) -> String {
    let escaped = path.display().to_string().replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cli_agent_command_uses_first_root_as_base_dir() {
        let roots = vec![
            PathBuf::from(r"C:\git\main project"),
            PathBuf::from(r"C:\git\shared"),
        ];

        let command = build_cli_agent_command(&CliAgentSettings::default(), &roots).unwrap();

        assert_eq!(command.agent_name, "Claude Code");
        assert_eq!(command.base_dir, PathBuf::from(r"C:\git\main project"));
        assert_eq!(
            command.command,
            "cd \"C:\\git\\main project\"\nclaude --add-dir \"C:\\git\\shared\""
        );
        assert_eq!(
            command.preview,
            "cd \"C:\\git\\main project\"\nclaude\n  --add-dir \"C:\\git\\shared\""
        );
    }

    #[test]
    fn build_cli_agent_command_escapes_double_quotes() {
        let roots = vec![PathBuf::from("C:\\git\\quoted\" project")];

        let command = build_cli_agent_command(&CliAgentSettings::default(), &roots).unwrap();

        assert_eq!(
            command.command,
            "cd \"C:\\git\\quoted\\\" project\"\nclaude"
        );
    }

    #[test]
    fn build_cli_agent_command_requires_a_workspace_root() {
        let error = build_cli_agent_command(&CliAgentSettings::default(), &[]).unwrap_err();

        assert_eq!(error, "No workspace folder is available.");
    }

    #[test]
    fn build_cli_agent_command_rejects_empty_executable() {
        let settings = CliAgentSettings {
            executable: " ".to_owned(),
            ..CliAgentSettings::default()
        };
        let error =
            build_cli_agent_command(&settings, &[PathBuf::from(r"C:\git\main")]).unwrap_err();

        assert_eq!(error, "AI command executable is empty.");
    }
}
