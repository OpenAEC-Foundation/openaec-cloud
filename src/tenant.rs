//! Multi-tenant configuration loader.
//!
//! Loads tenant definitions from `tenants.json` and resolves service account
//! passwords from environment variables.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use tracing::{info, warn};

/// Default path to the tenants configuration file.
const DEFAULT_CONFIG_PATH: &str = "/etc/openaec/tenants.json";

/// Configuration for a single tenant.
#[derive(Debug, Clone)]
pub struct TenantConfig {
    /// Short identifier (e.g. `"3bm"`).
    pub slug: String,
    /// Display name (e.g. `"3BM Cooperatie"`).
    pub name: String,
    /// Internal Nextcloud URL (e.g. `"http://nc-3bm:80"`).
    pub nextcloud_url: String,
    /// Public Nextcloud domain (e.g. `"cloud-3bm.open-aec.com"`).
    pub nextcloud_domain: String,
    /// Service account username.
    pub service_user: String,
    /// Service account password (resolved from env var).
    pub service_pass: String,
    /// Group Folder ID in Nextcloud (default: 1).
    pub group_folder_id: u32,
    /// Volume mount path (e.g. `"/nc-data-3bm"`).
    pub volume_mount: String,
}

impl TenantConfig {
    /// Path to the Group Folder files root on the volume mount.
    pub fn projects_root(&self) -> std::path::PathBuf {
        Path::new(&self.volume_mount)
            .join("__groupfolders")
            .join(self.group_folder_id.to_string())
            .join("files")
    }

    /// Check if the volume mount is accessible.
    pub fn has_volume_mount(&self) -> bool {
        !self.volume_mount.is_empty() && self.projects_root().is_dir()
    }
}

/// Registry of all configured tenants.
#[derive(Debug, Clone, Default)]
pub struct TenantsRegistry {
    tenants: HashMap<String, TenantConfig>,
}

impl TenantsRegistry {
    /// Load tenants from the path specified in `TENANTS_CONFIG` env var,
    /// or the default path `/etc/openaec/tenants.json`.
    pub fn load_from_env() -> Result<Self, String> {
        let path = std::env::var("TENANTS_CONFIG")
            .unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
        Self::load(&path)
    }

    /// Load tenants from a specific JSON file path.
    pub fn load(path: &str) -> Result<Self, String> {
        let path = Path::new(path);
        if !path.is_file() {
            info!(?path, "no tenants config — multi-tenant disabled");
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read tenants config: {e}"))?;

        let raw: RawConfig = serde_json::from_str(&contents)
            .map_err(|e| format!("invalid tenants JSON: {e}"))?;

        let mut tenants = HashMap::new();

        for (slug, data) in raw.tenants {
            // Resolve password from environment
            let pass_env = &data.service_pass_env;
            let service_pass = match std::env::var(pass_env) {
                Ok(pass) if !pass.is_empty() => pass,
                _ => {
                    warn!(
                        tenant = %slug,
                        env_var = %pass_env,
                        "service password not set — cloud storage disabled for this tenant"
                    );
                    continue;
                }
            };

            let volume_mount = format!("/nc-data-{slug}");

            let tenant = TenantConfig {
                slug: slug.clone(),
                name: data.name.unwrap_or_else(|| slug.clone()),
                nextcloud_url: data.nextcloud_url,
                nextcloud_domain: data.nextcloud_domain.unwrap_or_default(),
                service_user: data.service_user.unwrap_or_else(|| "openaec-service".to_string()),
                service_pass,
                group_folder_id: data.group_folder_id.unwrap_or(1),
                volume_mount: volume_mount.clone(),
            };

            info!(
                tenant = %slug,
                nextcloud_url = %tenant.nextcloud_url,
                volume = %volume_mount,
                mounted = tenant.has_volume_mount(),
                "loaded tenant"
            );

            tenants.insert(slug, tenant);
        }

        info!(count = tenants.len(), "loaded tenant configurations");
        Ok(Self { tenants })
    }

    /// Get a tenant by slug.
    pub fn get(&self, slug: &str) -> Option<&TenantConfig> {
        self.tenants.get(slug)
    }

    /// Get a tenant by slug, or return an error.
    pub fn get_or_err(&self, slug: &str) -> Result<&TenantConfig, crate::CloudError> {
        self.tenants
            .get(slug)
            .ok_or_else(|| crate::CloudError::NotFound(format!("unknown tenant: {slug}")))
    }

    /// List all tenant slugs.
    pub fn slugs(&self) -> Vec<&str> {
        self.tenants.keys().map(|s| s.as_str()).collect()
    }

    /// Check if any tenants are configured.
    pub fn is_configured(&self) -> bool {
        !self.tenants.is_empty()
    }

    /// Iterate over all tenants.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &TenantConfig)> {
        self.tenants.iter().map(|(k, v)| (k.as_str(), v))
    }
}

// ── Raw JSON deserialization ──────────────────────────────────

#[derive(Deserialize)]
struct RawConfig {
    tenants: HashMap<String, RawTenant>,
}

#[derive(Deserialize)]
struct RawTenant {
    name: Option<String>,
    nextcloud_url: String,
    nextcloud_domain: Option<String>,
    service_user: Option<String>,
    service_pass_env: String,
    group_folder_id: Option<u32>,
}
