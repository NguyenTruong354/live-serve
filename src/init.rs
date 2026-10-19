//! `live-serve init <editor>`: writes an editor task that starts live-serve for a project.

use std::path::{Path, PathBuf};

/// Editors with a task file live-serve can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    Zed,
    VsCode,
}

impl Editor {
    /// `zed`, or `vscode` / `code` (any case).
    pub fn parse(name: &str) -> Option<Editor> {
        match name.to_ascii_lowercase().as_str() {
            "zed" => Some(Editor::Zed),
            "vscode" | "code" => Some(Editor::VsCode),
            _ => None,
        }
    }
}

/// Marks a task file that already starts live-serve.
const MARKER: &str = "\"live-serve\"";

/// Result of `init`.
#[derive(Debug, PartialEq, Eq)]
pub enum InitOutcome {
    /// The task file was written.
    Created(PathBuf),
    /// The task file already has a live-serve task; nothing written.
    AlreadyThere(PathBuf),
    /// A task file exists with other tasks; left untouched (add the snippet by hand).
    Exists(PathBuf),
}

/// Where the editor reads project tasks from.
pub fn tasks_path(editor: Editor, project: &Path) -> PathBuf {
    match editor {
        Editor::Zed => project.join(".zed").join("tasks.json"),
        Editor::VsCode => project.join(".vscode").join("tasks.json"),
    }
}

/// Task file contents for `editor`, serving the project on `port`.
pub fn tasks_json(editor: Editor, port: u16) -> String {
    match editor {
        Editor::Zed => format!(
            r#"[
  {{
    "label": "live-serve ({port})",
    "command": "live-serve",
    "args": ["$ZED_WORKTREE_ROOT", "--port", "{port}"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "reveal": "no_focus",
    "hide": "never"
  }},
  {{
    "label": "live-serve ({port}, open browser)",
    "command": "live-serve",
    "args": ["$ZED_WORKTREE_ROOT", "--port", "{port}", "--open"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "reveal": "no_focus",
    "hide": "never"
  }}
]
"#
        ),
        // A background task: VS Code treats it as started once the "live-serve: http" line prints,
        // so it can also be a preLaunchTask.
        Editor::VsCode => format!(
            r#"{{
  "version": "2.0.0",
  "tasks": [
    {{
      "label": "live-serve ({port})",
      "type": "process",
      "command": "live-serve",
      "args": ["${{workspaceFolder}}", "--port", "{port}"],
      "isBackground": true,
      "presentation": {{ "reveal": "silent", "panel": "dedicated" }},
      "problemMatcher": {{
        "owner": "live-serve",
        "pattern": {{ "regexp": "^live-serve: error: (.*)$", "message": 1 }},
        "background": {{
          "activelyBegins": true,
          "beginsPattern": "^live-serve: starting",
          "endsPattern": "^live-serve: http"
        }}
      }}
    }}
  ]
}}
"#
        ),
    }
}

/// Write the task file for `editor` in `project`, never overwriting an existing one.
pub fn init(editor: Editor, project: &Path, port: u16) -> std::io::Result<InitOutcome> {
    let path = tasks_path(editor, project);
    if path.exists() {
        let current = std::fs::read_to_string(&path)?;
        return Ok(if current.contains(MARKER) {
            InitOutcome::AlreadyThere(path)
        } else {
            InitOutcome::Exists(path)
        });
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, tasks_json(editor, port))?;
    Ok(InitOutcome::Created(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("live-serve-init-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_editor_names() {
        assert_eq!(Editor::parse("zed"), Some(Editor::Zed));
        assert_eq!(Editor::parse("vscode"), Some(Editor::VsCode));
        assert_eq!(Editor::parse("code"), Some(Editor::VsCode));
        assert_eq!(Editor::parse("VSCode"), Some(Editor::VsCode));
        assert_eq!(Editor::parse("vim"), None);
    }

    #[test]
    fn task_paths() {
        let p = Path::new("proj");
        assert_eq!(tasks_path(Editor::Zed, p), p.join(".zed").join("tasks.json"));
        assert_eq!(tasks_path(Editor::VsCode, p), p.join(".vscode").join("tasks.json"));
    }

    #[test]
    fn zed_tasks_are_valid_and_serve_the_worktree() {
        let v: Value = serde_json::from_str(&tasks_json(Editor::Zed, 5500)).unwrap();
        let task = &v.as_array().unwrap()[0];
        assert_eq!(task["command"], "live-serve");
        assert!(task["label"].as_str().unwrap().contains("live-serve"));
        let args: Vec<&str> = task["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(args, ["$ZED_WORKTREE_ROOT", "--port", "5500"]);
    }

    #[test]
    fn vscode_tasks_are_valid_background_process_tasks() {
        let v: Value = serde_json::from_str(&tasks_json(Editor::VsCode, 8080)).unwrap();
        assert_eq!(v["version"], "2.0.0");
        let task = &v["tasks"].as_array().unwrap()[0];
        assert_eq!(task["type"], "process");
        assert_eq!(task["command"], "live-serve");
        assert_eq!(task["isBackground"], true);
        let args: Vec<&str> = task["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(args, ["${workspaceFolder}", "--port", "8080"]);
    }

    #[test]
    fn init_creates_then_leaves_alone() {
        let dir = project("create");
        let path = tasks_path(Editor::VsCode, &dir);
        assert_eq!(init(Editor::VsCode, &dir, 5500).unwrap(), InitOutcome::Created(path.clone()));
        assert!(std::fs::read_to_string(&path).unwrap().contains("live-serve"));
        assert_eq!(init(Editor::VsCode, &dir, 5500).unwrap(), InitOutcome::AlreadyThere(path));
    }

    #[test]
    fn init_never_overwrites_other_tasks() {
        let dir = project("existing");
        let path = tasks_path(Editor::Zed, &dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"[{"label":"build","command":"npm"}]"#).unwrap();
        assert_eq!(init(Editor::Zed, &dir, 5500).unwrap(), InitOutcome::Exists(path.clone()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"[{"label":"build","command":"npm"}]"#);
    }
}
