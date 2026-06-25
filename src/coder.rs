// ── src/coder.rs : code-builder workspace (Power 1 of the roadmap) ───────────
//
// Jarvis writes real software here. Every project lives in its OWN folder under
// ~/jarvis-projects, isolated from the rest of the disk so generated code is
// findable and never scattered on the Desktop. The tools in tools.rs that start
// with `code_` all route through here:
//
//   code_new_project  -> make a workspace (optionally scaffold a toolchain)
//   code_write_file   -> write a file INSIDE a project (path-traversal guarded)
//   code_read_file    -> read a file from a project
//   code_list         -> show the project's file tree
//   code_exec         -> run a command with the project as the working dir
//                        (this is build, test, run, and git all at once)
//
// The self-correct loop is the existing agent loop: code_exec returns the real
// exit code + stdout + stderr, so the model reads a failure and fixes it.

use std::path::{Component, Path, PathBuf};

// All generated projects live under one root, isolated from everything else.
pub fn workspace_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("jarvis-projects")
}

// Turn any project name into a safe folder name: lowercase, alphanumerics and
// dashes only, no leading/trailing or repeated dashes.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true; // true so a leading separator is dropped
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "project".to_string() } else { out }
}

// The absolute directory for a named project (not created here).
pub fn project_dir(project: &str) -> PathBuf {
    workspace_root().join(slugify(project))
}

// Resolve a file path INSIDE a project, refusing anything that climbs out with
// `..` or an absolute path. This is the safety boundary for code_write_file.
pub fn safe_join(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches('/');
    let candidate = dir.join(rel);
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("ERROR: file path must stay inside the project (no '..')".to_string());
    }
    Ok(candidate)
}

// Best-effort language detection from the files present, so we can suggest the
// right build/test commands to the model.
pub fn detect_language(dir: &Path) -> &'static str {
    if dir.join("Cargo.toml").exists() {
        "rust"
    } else if dir.join("package.json").exists() {
        "node"
    } else if dir.join("pyproject.toml").exists() || dir.join("requirements.txt").exists() {
        "python"
    } else if dir.join("go.mod").exists() {
        "go"
    } else if dir.join("index.html").exists() {
        "web"
    } else {
        "unknown"
    }
}

// Suggested build + test commands per language, surfaced in tool output so the
// model knows what to run via code_exec.
pub fn hints(lang: &str) -> (&'static str, &'static str) {
    match lang {
        "rust" => ("cargo build", "cargo test"),
        "node" => ("npm install", "npm test"),
        "python" => ("pip install -r requirements.txt", "pytest"),
        "go" => ("go build ./...", "go test ./..."),
        "web" => ("(open index.html)", "(no tests)"),
        _ => ("(unknown — inspect the files)", "(unknown)"),
    }
}

// The command that scaffolds a fresh toolchain in `dir`, if any. None means we
// just create the folder (and tools.rs may drop a starter file in).
pub fn scaffold_command(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some("cargo init ."),
        "node" => Some("npm init -y"),
        "go" => Some("go mod init app"),
        _ => None, // python/web/unknown are scaffolded with a starter file
    }
}

// Render a project's file tree, skipping the noisy build/dependency dirs.
pub fn tree(dir: &Path) -> String {
    fn skip(name: &str) -> bool {
        matches!(name, "target" | "node_modules" | ".git" | "dist" | "build" | ".venv" | "__pycache__")
    }
    fn walk(dir: &Path, prefix: &str, out: &mut String, depth: usize) {
        if depth > 6 {
            return;
        }
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.flatten().collect(),
            Err(_) => return,
        };
        entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name().to_string_lossy().to_lowercase()));
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if skip(&name) {
                continue;
            }
            if e.path().is_dir() {
                out.push_str(&format!("{prefix}{name}/\n"));
                walk(&e.path(), &format!("{prefix}  "), out, depth + 1);
            } else {
                out.push_str(&format!("{prefix}{name}\n"));
            }
        }
    }
    let mut out = String::new();
    walk(dir, "", &mut out, 0);
    if out.is_empty() { "(empty)".to_string() } else { out }
}
