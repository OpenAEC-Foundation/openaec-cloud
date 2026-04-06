//! # openaec-cloud
//!
//! Shared Nextcloud cloud storage library for OpenAEC tools.
//!
//! ## Hybrid I/O model
//!
//! - **Reads:** Direct filesystem I/O from a read-only Docker volume mount
//!   of the Nextcloud data volume. Fast, no network overhead.
//! - **Writes:** Via Nextcloud WebDAV API so metadata, search index and
//!   versioning stay in sync.
//! - **Fallback:** If the volume mount is unavailable (e.g. local dev),
//!   reads fall back to WebDAV GET.
//!
//! ## Project container model
//!
//! Projects use a standardised directory layout with a `project.wefc`
//! manifest that tracks all tool outputs. Legacy projects using
//! `99_overige_documenten/{tool_slug}/` are supported via automatic fallback.
//!
//! ## Multi-tenant
//!
//! Each tenant has its own Nextcloud instance. Configuration is loaded from
//! `tenants.json`. The active tenant is determined from the OIDC token's
//! `tenant` claim.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use openaec_cloud::{CloudClient, TenantsRegistry};
//!
//! let registry = TenantsRegistry::load_from_env().unwrap();
//! let tenant = registry.get("3bm").unwrap();
//! let client = CloudClient::new(tenant, "bim-validator");
//!
//! // List projects (reads from volume mount)
//! let projects = client.list_projects();
//!
//! // Upload a file (writes to validation/ via WebDAV)
//! // client.upload_file("My Project", "report.json", bytes).await?;
//!
//! // Read/write manifest
//! // let manifest = client.read_manifest("My Project").await?;
//! ```

pub mod container;
pub mod manifest;
mod tenant;
mod volume;
mod webdav;
mod propfind;

pub use container::output_dir_for_tool;
pub use manifest::ProjectManifest;
pub use tenant::{TenantConfig, TenantsRegistry};
pub use volume::{VolumeFileInfo, VolumeProject, VolumeReader};
pub use webdav::{CloudFile, CloudProject, WebdavClient};

use serde::Serialize;
use tracing::warn;

/// Summary info for a `.wefc` manifest file.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestInfo {
    pub name: String,
    pub size: u64,
    pub last_modified: String,
}

/// Whether a project uses the new container structure or the legacy layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectStructure {
    /// `project.wefc` manifest present — new container model.
    New,
    /// `99_overige_documenten/` layout — legacy.
    Legacy,
}

/// Combined cloud client — reads from volume mount, writes via WebDAV.
///
/// This is the primary interface consumers should use.
pub struct CloudClient {
    pub volume: VolumeReader,
    pub webdav: WebdavClient,
    tool_slug: String,
    output_dir: String,
}

impl CloudClient {
    /// Create a new cloud client for a tenant.
    ///
    /// - `tenant`: Tenant configuration (from [`TenantsRegistry`]).
    /// - `tool_slug`: Tool identifier (e.g. `"bim-validator"`).
    ///   Automatically mapped to the correct output directory.
    pub fn new(tenant: &TenantConfig, tool_slug: &str) -> Self {
        Self {
            volume: VolumeReader::new(tenant),
            webdav: WebdavClient::new(
                &tenant.nextcloud_url,
                &tenant.service_user,
                &tenant.service_pass,
                tool_slug,
            ),
            output_dir: output_dir_for_tool(tool_slug).to_string(),
            tool_slug: tool_slug.to_string(),
        }
    }

    /// Check if cloud storage is available (either volume or WebDAV).
    pub async fn is_available(&self) -> bool {
        if self.volume.available() {
            return true;
        }
        self.webdav.test_connection().await.unwrap_or(false)
    }

    /// List project folders.
    ///
    /// Reads from volume mount if available, otherwise falls back to WebDAV.
    pub fn list_projects(&self) -> Vec<VolumeProject> {
        if self.volume.available() {
            return self.volume.list_projects();
        }
        // Fallback: caller should use webdav.list_projects().await
        vec![]
    }

    /// List projects via WebDAV (async fallback).
    pub async fn list_projects_webdav(&self) -> Result<Vec<CloudProject>, CloudError> {
        self.webdav.list_projects().await
    }

    /// List files in the project's output directory.
    ///
    /// Tries new path first, falls back to legacy `99_overige_documenten/`.
    pub fn list_files(&self, project: &str) -> Vec<VolumeFileInfo> {
        if !self.volume.available() {
            return vec![];
        }

        // Try new path first
        let files = self
            .volume
            .list_files_at(project, &self.output_dir);
        if !files.is_empty() {
            return files;
        }

        // Fallback to legacy
        let legacy = self
            .volume
            .list_tool_files(project, &self.tool_slug);
        if !legacy.is_empty() {
            warn!(
                project,
                tool = %self.tool_slug,
                "using legacy path 99_overige_documenten — project not yet migrated"
            );
        }
        legacy
    }

    /// List files at an arbitrary subpath within a project.
    pub fn list_files_at(&self, project: &str, subdir: &str) -> Vec<VolumeFileInfo> {
        if self.volume.available() {
            return self.volume.list_files_at(project, subdir);
        }
        vec![]
    }

    /// List IFC/BIM model files in the `models/` directory.
    pub fn list_models(&self, project: &str) -> Vec<VolumeFileInfo> {
        if self.volume.available() {
            return self.volume.list_models(project);
        }
        vec![]
    }

    /// Read a file from the volume mount.
    ///
    /// Returns `None` if the volume mount is unavailable or the file doesn't exist.
    /// Use [`download_file`](Self::download_file) as async fallback.
    pub fn read_file(
        &self,
        project: &str,
        subdir: &str,
        filename: &str,
    ) -> Option<Vec<u8>> {
        self.volume.read_file(project, subdir, filename)
    }

    /// Get the filesystem path to a file (for streaming large files).
    ///
    /// Returns `None` if volume mount is unavailable or file doesn't exist.
    pub fn file_path(
        &self,
        project: &str,
        subdir: &str,
        filename: &str,
    ) -> Option<std::path::PathBuf> {
        self.volume.file_path(project, subdir, filename)
    }

    /// Download a file via WebDAV (async fallback for reads).
    pub async fn download_file(
        &self,
        project: &str,
        filename: &str,
    ) -> Result<Vec<u8>, CloudError> {
        self.webdav.download_file(project, filename).await
    }

    /// Upload (create or overwrite) a file via WebDAV.
    ///
    /// Always writes to the new output directory.
    pub async fn upload_file(
        &self,
        project: &str,
        filename: &str,
        data: Vec<u8>,
    ) -> Result<(), CloudError> {
        self.webdav.upload_file(project, filename, data).await
    }

    /// Delete a file via WebDAV.
    pub async fn delete_file(
        &self,
        project: &str,
        filename: &str,
    ) -> Result<(), CloudError> {
        self.webdav.delete_file(project, filename).await
    }

    /// Check if a project exists on the volume mount.
    pub fn project_exists(&self, project: &str) -> bool {
        self.volume.project_exists(project)
    }

    // ── Manifest operations ──────────────────────────────────

    /// Read a specific `.wefc` manifest by filename.
    ///
    /// Tries volume mount first, falls back to WebDAV.
    /// Returns `None` if the manifest doesn't exist.
    pub async fn read_manifest(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<ProjectManifest>, CloudError> {
        // Try volume mount first
        if let Some(bytes) = self.volume.read_manifest(project, name) {
            return ProjectManifest::from_bytes(&bytes)
                .map(Some)
                .map_err(|e| {
                    CloudError::Nextcloud(format!("manifest parse error: {e}"))
                });
        }

        // Fallback to WebDAV
        match self.webdav.download_manifest(project, name).await? {
            Some(bytes) => ProjectManifest::from_bytes(&bytes)
                .map(Some)
                .map_err(|e| {
                    CloudError::Nextcloud(format!("manifest parse error: {e}"))
                }),
            None => Ok(None),
        }
    }

    /// Read the default manifest (`project.wefc`).
    ///
    /// Convenience wrapper for backward compatibility.
    pub async fn read_default_manifest(
        &self,
        project: &str,
    ) -> Result<Option<ProjectManifest>, CloudError> {
        self.read_manifest(project, container::DEFAULT_MANIFEST_FILENAME).await
    }

    /// Write a manifest to a specific `.wefc` file via WebDAV.
    pub async fn write_manifest(
        &self,
        project: &str,
        name: &str,
        manifest: &ProjectManifest,
    ) -> Result<(), CloudError> {
        let bytes = manifest
            .to_bytes()
            .map_err(|e| CloudError::Nextcloud(format!("manifest serialize error: {e}")))?;
        self.webdav.upload_manifest(project, name, bytes).await
    }

    /// Write the default manifest (`project.wefc`) via WebDAV.
    ///
    /// Convenience wrapper for backward compatibility.
    pub async fn write_default_manifest(
        &self,
        project: &str,
        manifest: &ProjectManifest,
    ) -> Result<(), CloudError> {
        self.write_manifest(project, container::DEFAULT_MANIFEST_FILENAME, manifest).await
    }

    /// Add or update an object in a specific manifest (read -> merge -> write).
    ///
    /// Creates the manifest if it doesn't exist yet.
    pub async fn upsert_manifest_object(
        &self,
        project: &str,
        name: &str,
        object: serde_json::Value,
    ) -> Result<(), CloudError> {
        let mut manifest = self
            .read_manifest(project, name)
            .await?
            .unwrap_or_else(|| ProjectManifest::new(&self.tool_slug));

        manifest.header.application = self.tool_slug.clone();
        manifest.add_or_update(object);
        self.write_manifest(project, name, &manifest).await
    }

    /// Add or update an object in the default manifest (`project.wefc`).
    ///
    /// Convenience wrapper for backward compatibility.
    pub async fn upsert_default_manifest_object(
        &self,
        project: &str,
        object: serde_json::Value,
    ) -> Result<(), CloudError> {
        self.upsert_manifest_object(project, container::DEFAULT_MANIFEST_FILENAME, object).await
    }

    /// List all `.wefc` manifest files in a project.
    ///
    /// Combines results from volume mount and WebDAV, deduplicating by name
    /// (volume mount takes priority for metadata).
    pub async fn list_manifests(
        &self,
        project: &str,
    ) -> Result<Vec<ManifestInfo>, CloudError> {
        let mut manifests = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Volume mount first (fast path)
        if self.volume.available() {
            for info in self.volume.list_manifests(project) {
                seen.insert(info.name.clone());
                manifests.push(info);
            }
        }

        // WebDAV fallback for any not found on volume
        match self.webdav.list_manifests(project).await {
            Ok(cloud_files) => {
                for cf in cloud_files {
                    if !seen.contains(&cf.name) {
                        manifests.push(ManifestInfo {
                            name: cf.name,
                            size: cf.size,
                            last_modified: cf.last_modified,
                        });
                    }
                }
            }
            Err(CloudError::NotFound(_)) => {} // project doesn't exist on WebDAV
            Err(e) => {
                // If volume had results, log warning but don't fail
                if manifests.is_empty() {
                    return Err(e);
                }
                warn!(project, error = %e, "WebDAV manifest listing failed, using volume results only");
            }
        }

        manifests.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(manifests)
    }

    /// Detect whether a project uses the new or legacy structure.
    ///
    /// Checks for **any** `.wefc` file (not just `project.wefc`).
    pub async fn detect_structure(
        &self,
        project: &str,
    ) -> ProjectStructure {
        // Check volume mount first — any .wefc file means new structure
        if self.volume.available() && !self.volume.list_manifests(project).is_empty() {
            return ProjectStructure::New;
        }

        // Check via WebDAV
        match self.webdav.list_manifests(project).await {
            Ok(manifests) if !manifests.is_empty() => ProjectStructure::New,
            _ => ProjectStructure::Legacy,
        }
    }
}

/// Errors from cloud operations.
#[derive(Debug)]
pub enum CloudError {
    /// Resource not found (project or file).
    NotFound(String),
    /// Nextcloud returned an error or is unreachable.
    Nextcloud(String),
    /// I/O error reading from volume mount.
    Io(std::io::Error),
}

impl std::fmt::Display for CloudError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloudError::NotFound(msg) => write!(f, "not found: {msg}"),
            CloudError::Nextcloud(msg) => write!(f, "nextcloud error: {msg}"),
            CloudError::Io(err) => write!(f, "io error: {err}"),
        }
    }
}

impl std::error::Error for CloudError {}

impl From<std::io::Error> for CloudError {
    fn from(err: std::io::Error) -> Self {
        CloudError::Io(err)
    }
}
