//! Nextcloud WebDAV client for write operations and read fallback.
//!
//! All write operations (PUT, DELETE, MKCOL) go through WebDAV so Nextcloud's
//! database, search index and versioning stay in sync.

use reqwest::Client;
use serde::Serialize;
use tracing::debug;

use crate::container::{self, DEFAULT_MANIFEST_FILENAME, LEGACY_SUBDIR, MANIFEST_EXTENSION};
use crate::propfind;
use crate::CloudError;

/// Root folder on Nextcloud where all projects live.
const PROJECTS_ROOT: &str = "Projects";

/// A file entry from a WebDAV directory listing.
#[derive(Debug, Serialize, Clone)]
pub struct CloudFile {
    pub name: String,
    pub size: u64,
    pub last_modified: String,
    pub content_type: String,
}

/// A project folder entry.
#[derive(Debug, Serialize, Clone)]
pub struct CloudProject {
    pub name: String,
}

/// Nextcloud WebDAV client for a specific tool.
#[derive(Debug, Clone)]
pub struct WebdavClient {
    client: Client,
    webdav_root: String,
    username: String,
    password: String,
    /// Original tool identifier (e.g. `"bim-validator"`).
    tool_slug: String,
    /// Resolved output directory (e.g. `"validation"`).
    output_dir: String,
}

impl WebdavClient {
    /// Create a new WebDAV client.
    ///
    /// The `tool_slug` is automatically mapped to the correct output directory
    /// via [`container::output_dir_for_tool`].
    pub fn new(base_url: &str, username: &str, password: &str, tool_slug: &str) -> Self {
        let base = base_url.trim_end_matches('/');
        let encoded_user = urlencoding::encode(username);
        Self {
            client: Client::new(),
            webdav_root: format!("{base}/remote.php/dav/files/{encoded_user}"),
            username: username.to_string(),
            password: password.to_string(),
            output_dir: container::output_dir_for_tool(tool_slug).to_string(),
            tool_slug: tool_slug.to_string(),
        }
    }

    /// Test if Nextcloud is reachable.
    pub async fn test_connection(&self) -> Result<bool, CloudError> {
        let url = format!("{}/{PROJECTS_ROOT}/", self.webdav_root);
        let resp = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("unreachable: {e}")))?;
        Ok(resp.status().is_success() || resp.status().as_u16() == 207)
    }

    /// List all project folders under Projects/.
    pub async fn list_projects(&self) -> Result<Vec<CloudProject>, CloudError> {
        let url = format!("{}/{PROJECTS_ROOT}/", self.webdav_root);
        let entries = self.propfind(&url).await?;
        Ok(entries
            .into_iter()
            .filter(|e| e.is_collection)
            .map(|e| CloudProject { name: e.name })
            .collect())
    }

    /// List files in a project's output directory.
    ///
    /// Tries the new path first (`Projects/{project}/{output_dir}/`), then
    /// falls back to legacy (`Projects/{project}/99_overige_documenten/{tool_slug}/`).
    pub async fn list_files(&self, project: &str) -> Result<Vec<CloudFile>, CloudError> {
        let path = self.output_path(project);
        let url = format!("{}/{path}/", self.webdav_root);
        let items = match self.propfind(&url).await {
            Ok(items) if !items.is_empty() => items,
            Ok(_) | Err(CloudError::NotFound(_)) => {
                // Fallback to legacy path
                debug!(project, "new path empty, trying legacy path");
                let legacy = self.legacy_path(project);
                let legacy_url = format!("{}/{legacy}/", self.webdav_root);
                match self.propfind(&legacy_url).await {
                    Ok(items) => items,
                    Err(CloudError::NotFound(_)) => return Ok(vec![]),
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };

        Ok(items
            .into_iter()
            .filter(|e| !e.is_collection)
            .map(|e| CloudFile {
                name: e.name,
                size: e.size,
                last_modified: e.last_modified,
                content_type: e.content_type,
            })
            .collect())
    }

    /// List entries at an arbitrary path within a project.
    pub async fn list_path(
        &self,
        project: &str,
        subpath: &str,
    ) -> Result<Vec<CloudFile>, CloudError> {
        let full = self.project_subpath(project, subpath);
        let url = format!("{}/{full}/", self.webdav_root);
        match self.propfind(&url).await {
            Ok(items) => Ok(items
                .into_iter()
                .map(|e| CloudFile {
                    name: e.name,
                    size: e.size,
                    last_modified: e.last_modified,
                    content_type: e.content_type,
                })
                .collect()),
            Err(CloudError::NotFound(_)) => Ok(vec![]),
            Err(e) => Err(e),
        }
    }

    /// Download a file from the project's output directory.
    ///
    /// Tries new path first, falls back to legacy.
    pub async fn download_file(
        &self,
        project: &str,
        filename: &str,
    ) -> Result<Vec<u8>, CloudError> {
        let path = self.output_path(project);
        let encoded = urlencoding::encode(filename);
        let url = format!("{}/{path}/{encoded}", self.webdav_root);
        match self.get_bytes(&url, filename).await {
            Ok(bytes) => Ok(bytes),
            Err(CloudError::NotFound(_)) => {
                // Fallback to legacy path
                let legacy = self.legacy_path(project);
                let legacy_url = format!("{}/{legacy}/{encoded}", self.webdav_root);
                self.get_bytes(&legacy_url, filename).await
            }
            Err(e) => Err(e),
        }
    }

    /// Download a file at an arbitrary project subpath.
    pub async fn download_at(
        &self,
        project: &str,
        subpath: &str,
    ) -> Result<(Vec<u8>, String), CloudError> {
        let full = self.project_subpath(project, subpath);
        let url = format!("{}/{full}", self.webdav_root);

        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("download failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(CloudError::NotFound(format!("not found: {subpath}")));
        }
        if !resp.status().is_success() {
            return Err(CloudError::Nextcloud(format!("error: {}", resp.status())));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let bytes = resp
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| CloudError::Nextcloud(format!("read error: {e}")))?;

        Ok((bytes, content_type))
    }

    /// Upload a file to the project's output directory.
    ///
    /// Always writes to the NEW path (`Projects/{project}/{output_dir}/`).
    /// Creates intermediate directories via MKCOL if needed.
    pub async fn upload_file(
        &self,
        project: &str,
        filename: &str,
        data: Vec<u8>,
    ) -> Result<(), CloudError> {
        self.ensure_output_dir(project).await?;

        let path = self.output_path(project);
        let encoded = urlencoding::encode(filename);
        let url = format!("{}/{path}/{encoded}", self.webdav_root);

        debug!(project, filename, size = data.len(), "uploading via WebDAV");

        let resp = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .body(data)
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("upload failed: {e}")))?;

        let status = resp.status().as_u16();
        if status != 201 && status != 204 && !resp.status().is_success() {
            return Err(CloudError::Nextcloud(format!("upload error: {status}")));
        }

        Ok(())
    }

    /// Upload a file at an arbitrary project subpath.
    pub async fn upload_at(
        &self,
        project: &str,
        subpath: &str,
        data: Vec<u8>,
    ) -> Result<(), CloudError> {
        // Ensure parent directories
        self.ensure_parent_dirs(project, subpath).await?;

        let full = self.project_subpath(project, subpath);
        let url = format!("{}/{full}", self.webdav_root);

        let resp = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .body(data)
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("upload failed: {e}")))?;

        let status = resp.status().as_u16();
        if status != 201 && status != 204 && !resp.status().is_success() {
            return Err(CloudError::Nextcloud(format!("upload error: {status}")));
        }

        Ok(())
    }

    /// Delete a file from the project's output directory.
    ///
    /// Tries new path first, falls back to legacy.
    pub async fn delete_file(
        &self,
        project: &str,
        filename: &str,
    ) -> Result<(), CloudError> {
        let path = self.output_path(project);
        let encoded = urlencoding::encode(filename);
        let url = format!("{}/{path}/{encoded}", self.webdav_root);

        let resp = self
            .client
            .delete(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("delete failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(CloudError::NotFound(format!("not found: {filename}")));
        }

        Ok(())
    }

    /// Delete at an arbitrary project subpath.
    pub async fn delete_at(
        &self,
        project: &str,
        subpath: &str,
    ) -> Result<(), CloudError> {
        let full = self.project_subpath(project, subpath);
        let url = format!("{}/{full}", self.webdav_root);

        let resp = self
            .client
            .delete(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("delete failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(CloudError::NotFound(format!("not found: {subpath}")));
        }

        Ok(())
    }

    /// Create a directory at an arbitrary project subpath.
    pub async fn mkdir(
        &self,
        project: &str,
        subpath: &str,
    ) -> Result<(), CloudError> {
        let full = self.project_subpath(project, subpath);
        self.mkcol(&full).await
    }

    // ── Public: manifest helpers ─────────────────────────────

    /// Download a specific `.wefc` manifest by name via WebDAV.
    pub async fn download_manifest(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<Vec<u8>>, CloudError> {
        let safe = urlencoding::encode(project);
        let safe_name = urlencoding::encode(name);
        let url = format!(
            "{}/{PROJECTS_ROOT}/{safe}/{safe_name}",
            self.webdav_root
        );
        match self.get_bytes(&url, name).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(CloudError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Download the default manifest (`project.wefc`) via WebDAV.
    pub async fn download_default_manifest(
        &self,
        project: &str,
    ) -> Result<Option<Vec<u8>>, CloudError> {
        self.download_manifest(project, DEFAULT_MANIFEST_FILENAME).await
    }

    /// Upload (overwrite) a specific `.wefc` manifest via WebDAV.
    pub async fn upload_manifest(
        &self,
        project: &str,
        name: &str,
        data: Vec<u8>,
    ) -> Result<(), CloudError> {
        let safe = urlencoding::encode(project);
        let safe_name = urlencoding::encode(name);
        let url = format!(
            "{}/{PROJECTS_ROOT}/{safe}/{safe_name}",
            self.webdav_root
        );
        let resp = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .body(data)
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("manifest upload failed: {e}")))?;

        let status = resp.status().as_u16();
        if status != 201 && status != 204 && !resp.status().is_success() {
            return Err(CloudError::Nextcloud(format!(
                "manifest upload error: {status}"
            )));
        }
        Ok(())
    }

    /// Upload the default manifest (`project.wefc`) via WebDAV.
    pub async fn upload_default_manifest(
        &self,
        project: &str,
        data: Vec<u8>,
    ) -> Result<(), CloudError> {
        self.upload_manifest(project, DEFAULT_MANIFEST_FILENAME, data).await
    }

    /// Check if a specific `.wefc` manifest exists.
    pub async fn has_manifest(&self, project: &str, name: &str) -> bool {
        self.download_manifest(project, name)
            .await
            .map(|opt| opt.is_some())
            .unwrap_or(false)
    }

    /// Check if the default manifest (`project.wefc`) exists.
    pub async fn has_default_manifest(&self, project: &str) -> bool {
        self.has_manifest(project, DEFAULT_MANIFEST_FILENAME).await
    }

    /// List all `.wefc` manifest files in a project root via PROPFIND.
    pub async fn list_manifests(
        &self,
        project: &str,
    ) -> Result<Vec<CloudFile>, CloudError> {
        let safe = urlencoding::encode(project);
        let url = format!("{}/{PROJECTS_ROOT}/{safe}/", self.webdav_root);
        let entries = match self.propfind(&url).await {
            Ok(items) => items,
            Err(CloudError::NotFound(_)) => return Ok(vec![]),
            Err(e) => return Err(e),
        };

        Ok(entries
            .into_iter()
            .filter(|e| !e.is_collection && e.name.ends_with(MANIFEST_EXTENSION))
            .map(|e| CloudFile {
                name: e.name,
                size: e.size,
                last_modified: e.last_modified,
                content_type: e.content_type,
            })
            .collect())
    }

    // ── Private ──────────────────────────────────────────────

    /// New output path: `Projects/{project}/{output_dir}`
    fn output_path(&self, project: &str) -> String {
        let safe = urlencoding::encode(project);
        format!("{PROJECTS_ROOT}/{safe}/{}", self.output_dir)
    }

    /// Legacy path: `Projects/{project}/99_overige_documenten/{tool_slug}`
    fn legacy_path(&self, project: &str) -> String {
        let safe = urlencoding::encode(project);
        format!(
            "{PROJECTS_ROOT}/{safe}/{LEGACY_SUBDIR}/{}",
            self.tool_slug
        )
    }

    /// Arbitrary project subpath: `Projects/{project}/{subpath}`
    fn project_subpath(&self, project: &str, subpath: &str) -> String {
        let safe_project = urlencoding::encode(project);
        let trimmed = subpath.trim_matches('/');
        if trimmed.is_empty() {
            format!("{PROJECTS_ROOT}/{safe_project}")
        } else {
            let encoded_segments: Vec<String> = trimmed
                .split('/')
                .map(|seg| urlencoding::encode(seg).into_owned())
                .collect();
            format!(
                "{PROJECTS_ROOT}/{safe_project}/{}",
                encoded_segments.join("/")
            )
        }
    }

    /// Ensure the output directory hierarchy exists (new structure).
    async fn ensure_output_dir(&self, project: &str) -> Result<(), CloudError> {
        let safe = urlencoding::encode(project);
        let segments = [
            PROJECTS_ROOT.to_string(),
            format!("{PROJECTS_ROOT}/{safe}"),
            format!("{PROJECTS_ROOT}/{safe}/{}", self.output_dir),
        ];

        for seg in &segments {
            self.mkcol(seg).await?;
        }

        Ok(())
    }

    /// Ensure all parent directories for a file subpath exist.
    async fn ensure_parent_dirs(
        &self,
        project: &str,
        file_path: &str,
    ) -> Result<(), CloudError> {
        let trimmed = file_path.trim_matches('/');
        let parts: Vec<&str> = trimmed.split('/').collect();
        if parts.len() <= 1 {
            return Ok(());
        }

        let safe_project = urlencoding::encode(project);
        let mut cumulative = format!("{PROJECTS_ROOT}/{safe_project}");

        // Create each segment except the last (filename)
        for segment in &parts[..parts.len() - 1] {
            let encoded = urlencoding::encode(segment);
            cumulative = format!("{cumulative}/{encoded}");
            self.mkcol(&cumulative).await?;
        }

        Ok(())
    }

    /// Create a single directory via MKCOL.
    async fn mkcol(&self, path: &str) -> Result<(), CloudError> {
        let url = format!("{}/{path}/", self.webdav_root);
        let resp = self
            .client
            .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("mkcol failed: {e}")))?;

        let status = resp.status().as_u16();
        // 201 = created, 405 = already exists — both OK
        if status != 201 && status != 405 && !resp.status().is_success() {
            return Err(CloudError::Nextcloud(format!(
                "mkcol {path} failed: {status}"
            )));
        }

        Ok(())
    }

    /// GET bytes from a URL.
    async fn get_bytes(&self, url: &str, name: &str) -> Result<Vec<u8>, CloudError> {
        let resp = self
            .client
            .get(url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("download failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(CloudError::NotFound(format!("not found: {name}")));
        }
        if !resp.status().is_success() {
            return Err(CloudError::Nextcloud(format!("error: {}", resp.status())));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| CloudError::Nextcloud(format!("read error: {e}")))
    }

    /// PROPFIND a URL and parse the multistatus response.
    async fn propfind(
        &self,
        url: &str,
    ) -> Result<Vec<propfind::DavEntry>, CloudError> {
        let resp = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .send()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("unreachable: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(CloudError::NotFound("path not found".to_string()));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| CloudError::Nextcloud(format!("read error: {e}")))?;

        propfind::parse_propfind_xml(&body)
    }
}
