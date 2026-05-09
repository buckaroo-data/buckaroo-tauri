//! Plugin configuration: how to find Python, which buckaroo backend to expect,
//! and runtime overrides (working dir, env, port range, restart policy).

use std::path::PathBuf;

/// Which buckaroo backend the integrator's app expects to be available in the
/// user's Python environment. Today this only affects diagnostic messages; in
/// the future we may inspect installed packages to fail fast at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Xorq,
    Pandas,
    Polars,
}

#[derive(Debug, Clone)]
pub struct BuckarooConfig {
    /// Which backend the user's Python is expected to provide.
    pub backend: BackendKind,
    /// Absolute path to the Python interpreter. If `None`, resolution order is:
    ///   1. `BUCKAROO_PYTHON` env var
    ///   2. `python3` on PATH
    pub python_path: Option<PathBuf>,
    /// Working directory for the spawned sidecar. Defaults to the app's data dir.
    pub working_dir: Option<PathBuf>,
    /// Extra env vars passed to the sidecar.
    pub env: Vec<(String, String)>,
    /// Override the buckaroo-server port. Defaults to 0 (OS-assigned).
    pub port: u16,
    /// Maximum number of automatic restarts on crash. Defaults to 3.
    pub max_restarts: u32,
    /// If set, the supervisor auto-calls /load with this path right after
    /// `sidecar:ready`. Useful for headless verification — host apps that drive
    /// load via UI should leave this `None`.
    pub autoload_path: Option<PathBuf>,
}

impl BuckarooConfig {
    /// Default config for an xorq-flavored host (xorq + datafusion in user's Python).
    pub fn xorq() -> Self {
        Self::for_backend(BackendKind::Xorq)
    }

    /// Default config for a pandas-flavored host.
    pub fn pandas() -> Self {
        Self::for_backend(BackendKind::Pandas)
    }

    /// Default config for a polars-flavored host.
    pub fn polars() -> Self {
        Self::for_backend(BackendKind::Polars)
    }

    fn for_backend(backend: BackendKind) -> Self {
        Self {
            backend,
            python_path: None,
            working_dir: None,
            env: Vec::new(),
            port: 0,
            max_restarts: 3,
            autoload_path: None,
        }
    }

    /// Auto-load this parquet/CSV path after the sidecar is ready, without
    /// any UI interaction. The supervisor calls /load itself and opens the
    /// internal WS. Useful for headless verification + smoke tests.
    pub fn with_autoload_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.autoload_path = Some(path.into());
        self
    }

    /// Override the Python interpreter path. Highest priority — overrides env
    /// vars and PATH lookup.
    pub fn with_python(mut self, path: impl Into<PathBuf>) -> Self {
        self.python_path = Some(path.into());
        self
    }

    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn with_max_restarts(mut self, n: u32) -> Self {
        self.max_restarts = n;
        self
    }

    /// Resolve the Python interpreter to spawn. Returns the first match of:
    ///   1. `with_python(...)` override
    ///   2. `BUCKAROO_PYTHON` env var
    ///   3. `which buckaroo-server` — sibling `python` of the script
    ///   4. `which buckaroo-table` — sibling `python` (older console script)
    ///   5. `python3` on PATH
    pub(crate) fn resolve_python(&self) -> Result<PathBuf, String> {
        if let Some(p) = &self.python_path {
            log::debug!(
                "resolve_python: using with_python override: {}",
                p.display()
            );
            return Ok(p.clone());
        }
        if let Ok(env) = std::env::var("BUCKAROO_PYTHON") {
            if !env.is_empty() {
                log::debug!("resolve_python: using BUCKAROO_PYTHON env: {}", env);
                return Ok(PathBuf::from(env));
            }
        }
        // Try `which` for buckaroo's console scripts. If found, the sibling
        // `python` in the same bin directory is the right interpreter — this
        // is the venv that has buckaroo installed.
        for script in ["buckaroo-server", "buckaroo-table"] {
            if let Some(python) = sibling_python_via_which(script) {
                log::debug!(
                    "resolve_python: derived from `which {}`: {}",
                    script,
                    python.display()
                );
                return Ok(python);
            }
        }
        log::debug!("resolve_python: falling back to python3 on PATH");
        Ok(PathBuf::from("python3"))
    }
}

/// Run `which <name>` (or `where` on Windows) and, if successful, return the
/// `python` or `python3` executable in the same `bin/` directory.
fn sibling_python_via_which(script: &str) -> Option<PathBuf> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let output = std::process::Command::new(cmd).arg(script).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path_str = String::from_utf8_lossy(&output.stdout);
    let path = path_str.lines().next()?.trim();
    if path.is_empty() {
        return None;
    }
    let bin_dir = std::path::Path::new(path).parent()?;
    for candidate in ["python", "python3"] {
        let p = bin_dir.join(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    None
}
