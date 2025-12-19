//! =============================================================================
//! Tsserver Provider
//! =============================================================================
//!
//! Responsible for locating `tsserver.js` (local node_modules, Yarn SDK, and
//! PATH/global fallbacks) and reporting metadata (TypeScript version,
//! plugin probe location).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

const MAX_NESTED_SEARCH_DEPTH: usize = 4;

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

/// Caching the workspace root and lazily resolving
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
    pub fn resolve(&mut self) -> Result<TsserverBinary, ProviderError> {
        if let Some(path) = self.find_local_node_modules() {
            self.reanchor_workspace_root(&path);
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
            self.reanchor_workspace_root(&path);
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
        .or_else(|| {
            find_nested_match(
                &self.workspace_root,
                &["node_modules", "typescript", "lib", "tsserver.js"],
                MAX_NESTED_SEARCH_DEPTH,
            )
        })
    }

    fn find_yarn_sdk(&self) -> Option<PathBuf> {
        find_upwards(
            &self.workspace_root,
            &[".yarn", "sdks", "typescript", "lib", "tsserver.js"],
        )
        .or_else(|| {
            find_nested_match(
                &self.workspace_root,
                &[".yarn", "sdks", "typescript", "lib", "tsserver.js"],
                MAX_NESTED_SEARCH_DEPTH,
            )
        })
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

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    fn reanchor_workspace_root(&mut self, tsserver_js: &Path) {
        let Some(mut project_root) = project_root_from_tsserver(tsserver_js) else {
            return;
        };

        let should_update = self.workspace_root.starts_with(&project_root)
            || project_root.starts_with(&self.workspace_root);
        if !should_update {
            return;
        }

        if let Ok(canonical) = project_root.canonicalize() {
            project_root = canonical;
        }

        self.workspace_root = project_root;
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

fn project_root_from_tsserver(tsserver: &Path) -> Option<PathBuf> {
    let project = tsserver
        .parent()? // lib
        .parent()? // typescript
        .parent()? // node_modules or sdks
        .parent()?; // project root
    Some(project.to_path_buf())
}

fn find_nested_match(start: &Path, segments: &[&str], max_depth: usize) -> Option<PathBuf> {
    fn helper(dir: &Path, segments: &[&str], depth: usize, max_depth: usize) -> Option<PathBuf> {
        if depth > max_depth {
            return None;
        }

        let candidate = segments
            .iter()
            .fold(PathBuf::from(dir), |mut acc, segment| {
                acc.push(segment);
                acc
            });
        if candidate.is_file() {
            return Some(candidate);
        }
        if depth == max_depth {
            return None;
        }

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return None,
        };

        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            if let Some(name_str) = name.to_str() {
                if should_skip_dir(name_str) {
                    continue;
                }
            }
            if let Some(found) = helper(&entry.path(), segments, depth.saturating_add(1), max_depth)
            {
                return Some(found);
            }
        }

        None
    }

    helper(start, segments, 0, max_depth)
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | ".git"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".turbo"
            | ".pnpm"
            | "vendor"
    )
}
