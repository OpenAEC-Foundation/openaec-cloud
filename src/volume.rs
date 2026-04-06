//! Direct filesystem reader for Nextcloud volume mounts.
//!
//! Reads project listings and file contents directly from the read-only
//! Nextcloud data volume, bypassing WebDAV for fast I/O on large files.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::error;

use crate::container::{DEFAULT_MANIFEST_FILENAME, DIR_MODELS, LEGACY_SUBDIR, MANIFEST_EXTENSION};
use crate::ManifestInfo;
use crate::TenantConfig;

/// Metadata for a file on the volume mount.
#[derive(Debug, Clone)]
pub struct VolumeFileInfo {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    /// ISO 8601 timestamp.
    pub last_modified: String,
}

/// A project directory on the volume mount.
#[derive(Debug, Clone)]
pub struct VolumeProject {
    pub name: String,
    pub path: PathBuf,
}

/// Reads projects and files from a Nextcloud volume mount.
#[derive(Debug, Clone)]
pub struct VolumeReader {
    projects_root: PathBuf,
    is_available: bool,
}

impl VolumeReader {
    /// Create a reader from a tenant config.
    pub fn new(tenant: &TenantConfig) -> Self {
        let projects_root = tenant.projects_root();
        let is_available = tenant.has_volume_mount();
        Self {
            projects_root,
            is_available,
        }
    }

    /// Create a reader from an explicit path (for testing).
    pub fn from_path(projects_root: PathBuf) -> Self {
        let is_available = projects_root.is_dir();
        Self {
            projects_root,
            is_available,
        }
    }

    /// Check if the volume mount is accessible.
    pub fn available(&self) -> bool {
        self.is_available
    }

    /// List all project directories.
    pub fn list_projects(&self) -> Vec<VolumeProject> {
        if !self.is_available {
            return vec![];
        }

        let mut projects = Vec::new();
        let entries = match fs::read_dir(&self.projects_root) {
            Ok(entries) => entries,
            Err(e) => {
                error!(path = ?self.projects_root, error = %e, "failed to list projects");
                return vec![];
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if !name.starts_with('.') {
                        projects.push(VolumeProject {
                            name: name.to_string(),
                            path,
                        });
                    }
                }
            }
        }

        projects.sort_by(|a, b| a.name.cmp(&b.name));
        projects
    }

    /// List files in a project's tool-specific subdirectory (legacy path).
    ///
    /// Path: `{project}/99_overige_documenten/{tool_slug}/`
    pub fn list_tool_files(&self, project: &str, tool_slug: &str) -> Vec<VolumeFileInfo> {
        let dir = self
            .projects_root
            .join(project)
            .join(LEGACY_SUBDIR)
            .join(tool_slug);
        self.list_files_in(&dir, None)
    }

    /// List files at an arbitrary subpath within a project.
    pub fn list_files_at(&self, project: &str, subdir: &str) -> Vec<VolumeFileInfo> {
        let dir = self.projects_root.join(project).join(subdir);
        self.list_files_in(&dir, None)
    }

    /// List IFC/BIM model files in `{project}/models/`.
    pub fn list_models(&self, project: &str) -> Vec<VolumeFileInfo> {
        let dir = self.projects_root.join(project).join(DIR_MODELS);
        self.list_files_in(&dir, Some(&[".ifc", ".ifczip", ".ifcxml"]))
    }

    /// Read a specific `.wefc` manifest by filename.
    ///
    /// Returns `None` if the manifest doesn't exist or the volume is unavailable.
    pub fn read_manifest(&self, project: &str, name: &str) -> Option<Vec<u8>> {
        if !self.is_available {
            return None;
        }
        let path = self.projects_root.join(project).join(name);
        match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(_) => None,
        }
    }

    /// Read the default manifest (`project.wefc`) as raw bytes.
    ///
    /// Convenience wrapper around [`read_manifest`](Self::read_manifest).
    pub fn read_default_manifest(&self, project: &str) -> Option<Vec<u8>> {
        self.read_manifest(project, DEFAULT_MANIFEST_FILENAME)
    }

    /// List all `.wefc` manifest files in a project's root directory.
    pub fn list_manifests(&self, project: &str) -> Vec<ManifestInfo> {
        if !self.is_available {
            return vec![];
        }

        let dir = self.projects_root.join(project);
        if !dir.is_dir() {
            return vec![];
        }

        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                error!(path = ?dir, error = %e, "failed to list manifests");
                return vec![];
            }
        };

        let mut manifests = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match entry.file_name().to_str() {
                Some(n) if n.ends_with(MANIFEST_EXTENSION) => n.to_string(),
                _ => continue,
            };
            let (size, last_modified) = match entry.metadata() {
                Ok(meta) => {
                    let size = meta.len();
                    let modified = meta
                        .modified()
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    let duration = modified
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default();
                    (size, format_timestamp(duration.as_secs()))
                }
                Err(_) => (0, String::new()),
            };
            manifests.push(ManifestInfo {
                name,
                size,
                last_modified,
            });
        }

        manifests.sort_by(|a, b| a.name.cmp(&b.name));
        manifests
    }

    /// List files filtered by extensions (e.g. `[".ifc", ".ids"]`).
    pub fn list_files_filtered(
        &self,
        project: &str,
        subdir: &str,
        extensions: &[&str],
    ) -> Vec<VolumeFileInfo> {
        let dir = self.projects_root.join(project).join(subdir);
        self.list_files_in(&dir, Some(extensions))
    }

    /// Get the absolute path to a file, with path traversal protection.
    ///
    /// Returns `None` if the volume is unavailable, the file doesn't exist,
    /// or the path escapes the projects root.
    pub fn file_path(&self, project: &str, subdir: &str, filename: &str) -> Option<PathBuf> {
        if !self.is_available {
            return None;
        }

        let file_path = self.projects_root.join(project).join(subdir).join(filename);

        // Security: ensure path doesn't escape the projects root
        match file_path.canonicalize() {
            Ok(canonical) => {
                if let Ok(root_canonical) = self.projects_root.canonicalize() {
                    if !canonical.starts_with(&root_canonical) {
                        tracing::warn!(
                            path = ?file_path,
                            "path traversal attempt blocked"
                        );
                        return None;
                    }
                }
                if canonical.is_file() {
                    Some(canonical)
                } else {
                    None
                }
            }
            Err(_) => None, // File doesn't exist
        }
    }

    /// Read file content from the volume mount.
    ///
    /// Returns `None` if the file doesn't exist or volume is unavailable.
    pub fn read_file(&self, project: &str, subdir: &str, filename: &str) -> Option<Vec<u8>> {
        let path = self.file_path(project, subdir, filename)?;
        match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                error!(path = ?path, error = %e, "failed to read file");
                None
            }
        }
    }

    /// Check if a project directory exists.
    pub fn project_exists(&self, project: &str) -> bool {
        if !self.is_available {
            return false;
        }
        self.projects_root.join(project).is_dir()
    }

    // ── Private ──────────────────────────────────────────────

    fn list_files_in(
        &self,
        dir: &Path,
        extensions: Option<&[&str]>,
    ) -> Vec<VolumeFileInfo> {
        if !dir.is_dir() {
            return vec![];
        }

        let mut files = Vec::new();
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                error!(path = ?dir, error = %e, "failed to list directory");
                return vec![];
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let name = match entry.file_name().to_str() {
                Some(n) if !n.starts_with('.') => n.to_string(),
                _ => continue,
            };

            // Extension filter
            if let Some(exts) = extensions {
                let has_ext = exts.iter().any(|ext| {
                    name.to_lowercase().ends_with(&ext.to_lowercase())
                });
                if !has_ext {
                    continue;
                }
            }

            let (size, last_modified) = match entry.metadata() {
                Ok(meta) => {
                    let size = meta.len();
                    let modified = meta
                        .modified()
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    let duration = modified
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default();
                    let secs = duration.as_secs();
                    // Simple ISO 8601 without chrono dependency
                    let ts = format_timestamp(secs);
                    (size, ts)
                }
                Err(_) => (0, String::new()),
            };

            files.push(VolumeFileInfo {
                name,
                path,
                size,
                last_modified,
            });
        }

        files.sort_by(|a, b| a.name.cmp(&b.name));
        files
    }
}

/// Format a unix timestamp as ISO 8601 (basic, no chrono dependency).
fn format_timestamp(secs: u64) -> String {
    // Days/months calculation
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // Compute year/month/day from days since epoch
    let mut y = 1970i64;
    let mut remaining = days as i64;

    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }

    let months_days: [i64; 12] = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 0usize;
    for (i, &md) in months_days.iter().enumerate() {
        if remaining < md {
            m = i;
            break;
        }
        remaining -= md;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        remaining + 1,
        hours,
        minutes,
        seconds
    )
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
