//! Linux `bubblewrap` backend.
//!
//! The flag list is produced by a **pure** function ([`bwrap_args`]) so the
//! "only bind paths that exist" rule and the "`--new-session` must never be
//! passed" rule can be unit-tested without a sandbox on the host.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::Sandbox;

/// Read-only system paths bound into the sandbox when they exist on the host.
const OPTIONAL_SYSTEM_PATHS: &[&str] = &["/usr", "/bin", "/lib", "/etc"];

/// Read-only system paths that must exist: `/lib64` carries the dynamic loader
/// on glibc hosts, and without it every command fails with a misleading
/// `bwrap: execvp sh: No such file or directory`. Treated as a construction
/// failure rather than a silently skipped mount.
const REQUIRED_SYSTEM_PATHS: &[&str] = &["/lib64"];

/// Read-only toolchain homes, mounted at their **host** absolute paths.
///
/// `HOME` is `/workspace` inside the sandbox, so `~` no longer resolves here;
/// these directories are what make `cargo` (a rustup shim), `git commit` and
/// `npm` work at all. Tuple: (path relative to `$HOME`, environment variable,
/// optional path inside that directory the variable must point at).
///
/// Mounting at the host path means bubblewrap also creates an otherwise empty
/// `/home/<user>` mount point inside the sandbox — the homes are the only
/// entries under it, which is the documented allowlist (§17.1 of the sandbox
/// design), not an accidental home mount.
const TOOLCHAIN_HOMES: &[(&str, &str, &str)] = &[
    (".rustup", "RUSTUP_HOME", ""),
    (".cargo", "CARGO_HOME", ""),
    (".config/git", "GIT_CONFIG_GLOBAL", "config"),
    (".npm", "NPM_CONFIG_CACHE", ""),
];

/// Linux bubblewrap sandbox. Stateless: one process invocation is built per
/// `bash` call, matching the existing stateless `sh -c` semantics.
pub struct BwrapSandbox;

impl BwrapSandbox {
    /// Verify the whole policy once against a throwaway command.
    ///
    /// A mis-specified policy fails at *every* command with a misleading
    /// message (a missing mount looks like a missing shell; a restricted kernel
    /// looks like the user's command exiting 1), so the policy is exercised at
    /// startup and a failure is recorded as a degradation reason instead of
    /// surfacing per call.
    ///
    /// # Errors
    ///
    /// Returns the human-readable degradation reason when `bwrap` is absent or
    /// the probe command fails.
    pub fn probe(work_dir: &Path) -> Result<Self, String> {
        let args = bwrap_args(work_dir).map_err(|err| err.to_string())?;
        let output = match std::process::Command::new("bwrap")
            .args(&args)
            .arg("/bin/true")
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err("backend 'bwrap' not found on PATH".to_string());
            }
            Err(err) => return Err(format!("backend 'bwrap' could not be started: {err}")),
        };
        if output.status.success() {
            Ok(Self)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            if detail.is_empty() {
                Err(format!(
                    "backend 'bwrap' is unusable (probe exited {:?})",
                    output.status.code()
                ))
            } else {
                Err(format!("backend 'bwrap' is unusable: {detail}"))
            }
        }
    }
}

impl Sandbox for BwrapSandbox {
    fn command(
        &self,
        program: &str,
        args: &[String],
        work_dir: &Path,
    ) -> Result<tokio::process::Command> {
        let mut command = tokio::process::Command::new("bwrap");
        command.args(bwrap_args(work_dir)?);
        command.arg(program).args(args);
        Ok(command)
    }

    fn describe(&self) -> &'static str {
        "bwrap"
    }
}

/// The full policy flag list, ending with the `--` separator.
///
/// Existence is probed against the real filesystem; [`bwrap_args_with`] takes
/// the probe as a parameter so the layout rules are unit-testable.
///
/// # Errors
///
/// Returns an error when the workspace guard refuses `work_dir` or a required
/// host mount is missing.
pub fn bwrap_args(work_dir: &Path) -> Result<Vec<String>> {
    bwrap_args_with(work_dir, &|path| path.exists())
}

fn bwrap_args_with(work_dir: &Path, exists: &dyn Fn(&Path) -> bool) -> Result<Vec<String>> {
    let work_dir = guard_workspace(work_dir)?;
    let home = home_dir();

    let mut args = Vec::with_capacity(48);
    // `--new-session` is deliberately absent: it detaches the command into its
    // own process group, so the existing `killpg` teardown would kill only
    // bwrap while grandchildren hold the stdout/stderr pipes open — the call
    // would then never return. `--unshare-pid` gives stronger teardown (the pid
    // namespace dies with bwrap) and keeps the mounted `/proc` from showing the
    // host process table.
    push(
        &mut args,
        &["--die-with-parent", "--unshare-net", "--unshare-pid"],
    );

    // The workspace is the only host directory mounted read-write, and it is
    // not visible under its host path.
    push(&mut args, &["--bind"]);
    args.push(work_dir.display().to_string());
    push(&mut args, &["/workspace"]);

    for path in REQUIRED_SYSTEM_PATHS {
        let path = Path::new(path);
        if !exists(path) {
            anyhow::bail!(
                "missing required mount: {} (the dynamic loader lives there)",
                path.display()
            );
        }
        ro_bind(&mut args, path);
    }
    for path in OPTIONAL_SYSTEM_PATHS {
        let path = Path::new(path);
        if exists(path) {
            ro_bind(&mut args, path);
        } else {
            tracing::warn!(path = %path.display(), "sandbox: system path absent, not bound");
        }
    }

    let mut toolchain_env = Vec::new();
    if let Some(home) = &home {
        for (relative, variable, file) in TOOLCHAIN_HOMES {
            let host_path = home.join(relative);
            if !exists(&host_path) {
                tracing::debug!(path = %host_path.display(), "sandbox: toolchain home absent, not bound");
                continue;
            }
            ro_bind(&mut args, &host_path);
            let value = if file.is_empty() {
                host_path.display().to_string()
            } else {
                host_path.join(file).display().to_string()
            };
            toolchain_env.push(((*variable).to_string(), value));
        }
    }

    push(
        &mut args,
        &["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"],
    );
    push(&mut args, &["--chdir", "/workspace"]);

    // The host environment is not inherited: it leaks the host home (not
    // mounted) and proxy variables pointing at a listener that does not exist
    // inside the sandbox — both turn real failures into misleading ones.
    push(&mut args, &["--clearenv"]);
    setenv(&mut args, "PATH", "/usr/local/bin:/usr/bin:/bin");
    setenv(&mut args, "HOME", "/workspace");
    setenv(&mut args, "TERM", "dumb");
    for variable in ["LANG", "LC_ALL"] {
        if let Ok(value) = std::env::var(variable) {
            setenv(&mut args, variable, &value);
        }
    }
    for (variable, value) in &toolchain_env {
        setenv(&mut args, variable, value);
    }

    push(&mut args, &["--"]);
    Ok(args)
}

fn push(args: &mut Vec<String>, values: &[&str]) {
    args.extend(values.iter().map(|value| (*value).to_string()));
}

fn ro_bind(args: &mut Vec<String>, path: &Path) {
    let path = path.display().to_string();
    push(args, &["--ro-bind"]);
    args.push(path.clone());
    args.push(path);
}

fn setenv(args: &mut Vec<String>, variable: &str, value: &str) {
    push(args, &["--setenv", variable]);
    args.push(value.to_string());
}

fn home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(canonicalize_or_raw(Path::new(&home)))
}

/// Refuse workspace roots whose read-write bind would silently expose far more
/// than the project: `$HOME` (credentials), an ancestor of `$HOME`, the
/// filesystem root, or a system directory.
fn guard_workspace(work_dir: &Path) -> Result<PathBuf> {
    let work_dir = canonicalize_or_raw(work_dir);
    let display = work_dir.display();

    if work_dir == Path::new("/") {
        anyhow::bail!("refusing to sandbox: workspace {display} is the filesystem root");
    }
    if let Some(home) = home_dir() {
        if work_dir == home {
            anyhow::bail!(
                "refusing to sandbox: workspace {display} is the host home directory, \
                 which the workspace bind would expose read-write"
            );
        }
        if home.starts_with(&work_dir) {
            anyhow::bail!(
                "refusing to sandbox: workspace {display} is an ancestor of the host home \
                 directory, which the workspace bind would expose read-write"
            );
        }
    }
    for system in ["/etc", "/usr", "/boot", "/bin", "/lib", "/lib64"] {
        if work_dir.starts_with(system) {
            anyhow::bail!("refusing to sandbox: workspace {display} is a system directory");
        }
    }
    Ok(work_dir)
}

/// Resolve to an absolute, symlink-free path when possible; fall back to the
/// raw path (a relative workspace is made absolute against the current
/// directory) when it does not exist yet.
fn canonicalize_or_raw(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
        .components()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn everything_exists(_path: &Path) -> bool {
        true
    }

    fn exists_except(missing: &'static str) -> impl Fn(&Path) -> bool {
        move |path: &Path| path != Path::new(missing)
    }

    fn workspace() -> PathBuf {
        let dir = std::env::temp_dir().join("tact-bwrap-args-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn index_of(args: &[String], value: &str) -> usize {
        args.iter()
            .position(|arg| arg == value)
            .unwrap_or_else(|| panic!("missing {value} in {args:?}"))
    }

    #[test]
    fn emits_the_documented_flag_order() {
        let args = bwrap_args_with(&workspace(), &everything_exists).unwrap();

        assert_eq!(args[0], "--die-with-parent");
        assert_eq!(args[1], "--unshare-net");
        assert_eq!(args[2], "--unshare-pid");
        assert_eq!(args[3], "--bind");
        // The workspace is mounted at /workspace, never at its host path.
        assert_eq!(args[5], "/workspace");

        for path in ["/usr", "/bin", "/lib", "/lib64", "/etc"] {
            let at = index_of(&args, path);
            assert_eq!(args[at - 1], "--ro-bind", "{path} is not ro-bound");
        }

        assert_eq!(index_of(&args, "--proc") + 1, index_of(&args, "/proc"));
        assert_eq!(index_of(&args, "--dev") + 1, index_of(&args, "/dev"));
        assert_eq!(index_of(&args, "--tmpfs") + 1, index_of(&args, "/tmp"));
        // `/workspace` appears first as the bind destination, so compare the
        // argument that follows `--chdir` rather than searching for the string.
        assert_eq!(args[index_of(&args, "--chdir") + 1], "/workspace");

        // The separator is last; `sh -c <command>` is appended by the caller.
        assert_eq!(args.last().unwrap(), "--");
    }

    #[test]
    fn never_passes_new_session() {
        // Regression guard: `--new-session` detaches the command into its own
        // process group and breaks the existing killpg teardown.
        let args = bwrap_args_with(&workspace(), &everything_exists).unwrap();
        assert!(!args.iter().any(|arg| arg == "--new-session"));
    }

    #[test]
    fn missing_lib64_is_a_hard_error() {
        let error = bwrap_args_with(&workspace(), &exists_except("/lib64"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("/lib64"), "unexpected error: {error}");
    }

    #[test]
    fn absent_optional_system_path_is_skipped() {
        let args = bwrap_args_with(&workspace(), &exists_except("/etc")).unwrap();
        let etc_bind = args
            .windows(2)
            .any(|pair| pair[0] == "--ro-bind" && pair[1] == "/etc");
        assert!(!etc_bind, "/etc should be skipped when absent: {args:?}");
    }

    #[test]
    fn clears_the_environment_and_sets_an_allowlist() {
        let args = bwrap_args_with(&workspace(), &everything_exists).unwrap();

        let clearenv = index_of(&args, "--clearenv");
        let first_setenv = index_of(&args, "--setenv");
        assert!(clearenv < first_setenv, "--clearenv must precede --setenv");

        let home = index_of(&args, "HOME");
        assert_eq!(args[home + 1], "/workspace");
        let path = index_of(&args, "PATH");
        assert_eq!(args[path + 1], "/usr/local/bin:/usr/bin:/bin");

        // Proxy variables are not carried into the sandbox.
        for variable in ["http_proxy", "https_proxy", "all_proxy"] {
            assert!(!args.iter().any(|arg| arg == variable));
        }
    }

    #[test]
    fn guard_rejects_the_filesystem_root() {
        let error = bwrap_args_with(Path::new("/"), &everything_exists)
            .unwrap_err()
            .to_string();
        assert!(error.contains("filesystem root"), "unexpected: {error}");
    }

    #[test]
    fn guard_rejects_the_home_directory_and_its_ancestors() {
        let Some(home) = home_dir() else {
            return; // no $HOME on this host; nothing to assert
        };
        let error = bwrap_args_with(&home, &everything_exists)
            .unwrap_err()
            .to_string();
        assert!(error.contains("home directory"), "unexpected: {error}");

        if let Some(ancestor) = home.parent() {
            let error = bwrap_args_with(ancestor, &everything_exists)
                .unwrap_err()
                .to_string();
            assert!(error.contains("ancestor"), "unexpected: {error}");
        }
    }

    #[test]
    fn guard_rejects_system_directories() {
        for path in ["/etc", "/usr/lib", "/boot"] {
            assert!(
                bwrap_args_with(Path::new(path), &everything_exists).is_err(),
                "{path} should be refused"
            );
        }
    }

    #[test]
    fn guard_accepts_a_project_directory_under_the_home_directory() {
        // The common case: the workspace lives *inside* `$HOME` (e.g.
        // ~/Projects/tact). Only `$HOME` itself and its ancestors are refused;
        // a descendant is a normal workspace and must stay usable.
        let Some(home) = home_dir() else {
            return; // no $HOME on this host; nothing to assert
        };
        let project = home.join("Projects").join("sandbox-guard-probe");
        let args = bwrap_args_with(&project, &everything_exists).unwrap();
        let at = index_of(&args, "--bind");
        assert_eq!(args[at + 1], project.display().to_string());
    }
}
