//! =============================================================================
//! Tsserver Provider
//! =============================================================================
//!
//! Responsible for locating `tsserver.js` (local node_modules, Yarn SDK, and
//! PATH/global fallbacks) and reporting metadata (TypeScript version,
//! plugin probe location).  This mirrors the Lua provider but keeps the search
//! order deterministic and testable.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Captures everything needed to spawn a tsserver instance.
#[derive(Debug, Clone)]
pub struct TsserverBinary {
    pub executable: PathBuf,
    pub plugin_probe: Option<PathBuf>,
    pub version: Option<String>,
    pub source: BinarySource,
}

impl TsserverBinary {
    fn new(executable: PathBuf, plugin_probe: Option<PathBuf>, source: BinarySource) -> Self {
        let version = infer_version(&executable);
        Self {
            executable,
            plugin_probe,
            version,
            source,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BinarySource {
    LocalNodeModules,
    YarnSdk,
    GlobalPath,
}

/// Mirrors the Lua provider by caching the workspace root and lazily resolving
/// binaries when the RPC service boots up.
#[derive(Debug)]
pub struct Provider {
    workspace_root: PathBuf,
}

impl Provider {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let root = workspace_root
            .into()
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from("."));
        Self {
            workspace_root: root,
        }
    }

    /// Resolves the tsserver binary by inspecting (in order):
    /// 1. `node_modules/typescript/lib/tsserver.js` in workspace ancestors.
    /// 2. `.yarn/sdks/typescript/lib/tsserver.js` in ancestors.
    /// 3. `tsserver` on PATH (via `which`).
    pub fn resolve(&self) -> Result<TsserverBinary, ProviderError> {
        if let Some(path) = self.find_local_node_modules() {
            let plugin_probe = path
                .parent()
                .and_then(|lib| lib.parent())
                .and_then(|ts| ts.parent())
                .map(Path::to_path_buf);
            return Ok(TsserverBinary::new(
                path,
                plugin_probe,
                BinarySource::LocalNodeModules,
            ));
        }

        if let Some(path) = self.find_yarn_sdk() {
            let plugin_probe = path
                .parent()
                .and_then(|lib| lib.parent())
                .and_then(|ts| ts.parent())
                .map(Path::to_path_buf);
            return Ok(TsserverBinary::new(
                path,
                plugin_probe,
                BinarySource::YarnSdk,
            ));
        }

        if let Some(path) = self.find_global_tsserver()? {
            return Ok(TsserverBinary::new(path, None, BinarySource::GlobalPath));
        }

        Err(ProviderError::NotFound {
            root: self.workspace_root.clone(),
        })
    }

    fn find_local_node_modules(&self) -> Option<PathBuf> {
        find_upwards(
            &self.workspace_root,
            &["node_modules", "typescript", "lib", "tsserver.js"],
        )
    }

    fn find_yarn_sdk(&self) -> Option<PathBuf> {
        find_upwards(
            &self.workspace_root,
            &[".yarn", "sdks", "typescript", "lib", "tsserver.js"],
        )
    }

    fn find_global_tsserver(&self) -> Result<Option<PathBuf>, ProviderError> {
        match which::which("tsserver") {
            Ok(path) => {
                // Some distributions expose a wrapper script; if so try to backtrack to the JS file.
                if path.file_name().and_then(|f| f.to_str()) == Some("tsserver.js") {
                    Ok(Some(path))
                } else {
                    Ok(transform_wrapper_to_js(path))
                }
            }
            Err(which::Error::CannotFindBinaryPath) => Ok(None),
            Err(err) => Err(ProviderError::PathLookup(err)),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ProviderError {
    #[error("unable to locate tsserver starting at {root:?}")]
    NotFound { root: PathBuf },
    #[error("failed to invoke `which tsserver`: {0}")]
    PathLookup(which::Error),
}

fn transform_wrapper_to_js(wrapper: PathBuf) -> Option<PathBuf> {
    let mut candidate = wrapper.clone();
    candidate.pop(); // drop tsserver filename
    candidate.pop(); // drop bin/
    candidate.push("lib");
    candidate.push("node_modules");
    candidate.push("typescript");
    candidate.push("lib");
    candidate.push("tsserver.js");
    candidate.canonicalize().ok().filter(|path| path.exists())
}

fn find_upwards(start: &Path, segments: &[&str]) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = segments
            .iter()
            .fold(PathBuf::from(ancestor), |mut acc, segment| {
                acc.push(segment);
                acc
            });

        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

fn infer_version(tsserver: &Path) -> Option<String> {
    let lib_dir = tsserver.parent()?;
    let ts_dir = lib_dir.parent()?;
    let package_json = ts_dir.join("package.json");
    let contents = fs::read_to_string(package_json).ok()?;
    let json: Value = serde_json::from_str(&contents).ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
