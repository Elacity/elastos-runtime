use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use elastos_common::CapsuleManifest;
use elastos_runtime::provider::ProviderRegistry;

#[derive(Debug, Clone)]
pub struct InspectEntry {
    pub id: String,
    pub name: String,
    pub status: String,
    pub capsule_type: String,
    pub manifest: Option<CapsuleManifest>,
    pub cid: Option<String>,
}

#[async_trait]
pub trait InspectSource: Send + Sync {
    async fn inspect_list(&self) -> Vec<InspectEntry>;
    async fn inspect_get(&self, id: &str) -> Option<InspectEntry>;
}

pub struct RegistryInspectSource {
    registry: Weak<ProviderRegistry>,
}

impl RegistryInspectSource {
    pub fn new(registry: Weak<ProviderRegistry>) -> Self {
        Self { registry }
    }

    fn scheme_entry(scheme: String) -> InspectEntry {
        InspectEntry {
            id: format!("provider:{scheme}"),
            name: scheme,
            status: "running".to_string(),
            capsule_type: "provider".to_string(),
            manifest: None,
            cid: None,
        }
    }
}

#[async_trait]
impl InspectSource for RegistryInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let Some(registry) = self.registry.upgrade() else {
            return Vec::new();
        };
        let mut schemes = registry.schemes().await;
        schemes.extend(registry.sub_provider_schemes().await);
        schemes.sort();
        schemes.dedup();
        schemes.into_iter().map(Self::scheme_entry).collect()
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let scheme = id.strip_prefix("provider:")?;
        let registry = self.registry.upgrade()?;
        let is_known = registry.has_provider(scheme).await
            || registry
                .sub_provider_schemes()
                .await
                .iter()
                .any(|known| known == scheme);
        is_known.then(|| Self::scheme_entry(scheme.to_string()))
    }
}

pub struct CatalogInspectSource {
    capsules_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
}

impl CatalogInspectSource {
    pub fn new(capsules_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self {
            capsules_dir,
            registry,
        }
    }

    fn provided_scheme(manifest: &CapsuleManifest) -> Option<String> {
        manifest
            .provides
            .as_ref()?
            .strip_prefix("elastos://")
            .and_then(|rest| rest.split('/').next())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    async fn running_schemes(&self) -> HashSet<String> {
        let Some(registry) = self.registry.upgrade() else {
            return HashSet::new();
        };
        let mut schemes: HashSet<String> = registry.schemes().await.into_iter().collect();
        schemes.extend(registry.sub_provider_schemes().await);
        schemes
    }

    async fn catalog_cids(&self) -> HashMap<String, String> {
        let Some(path) = self
            .capsules_dir
            .parent()
            .map(|path| path.join("components.json"))
        else {
            return HashMap::new();
        };
        let Ok(data) = tokio::fs::read_to_string(path).await else {
            return HashMap::new();
        };
        serde_json::from_str::<crate::setup::ComponentsManifest>(&data)
            .map(|manifest| {
                manifest
                    .capsules
                    .into_iter()
                    .filter(|(_, entry)| !entry.cid.is_empty())
                    .map(|(name, entry)| (name, entry.cid))
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn read_entry(
        &self,
        name: &str,
        running: &HashSet<String>,
        cid: Option<String>,
    ) -> Option<InspectEntry> {
        if name.contains('/') || name.contains("..") {
            return None;
        }
        let path = self.capsules_dir.join(name).join("capsule.json");
        let data = tokio::fs::read_to_string(path).await.ok()?;
        let manifest: CapsuleManifest = serde_json::from_str(&data).ok()?;
        let is_running = Self::provided_scheme(&manifest)
            .map(|scheme| running.contains(&scheme))
            .unwrap_or(false);
        Some(InspectEntry {
            id: format!("capsule:{name}"),
            name: manifest.name.clone(),
            status: if is_running { "running" } else { "installed" }.to_string(),
            capsule_type: format!("{:?}", manifest.capsule_type).to_lowercase(),
            manifest: Some(manifest),
            cid,
        })
    }
}

#[async_trait]
impl InspectSource for CatalogInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let running = self.running_schemes().await;
        let cids = self.catalog_cids().await;
        let mut entries = Vec::new();
        let Ok(mut dirs) = tokio::fs::read_dir(&self.capsules_dir).await else {
            return entries;
        };
        while let Ok(Some(dir)) = dirs.next_entry().await {
            let is_dir = dir.file_type().await.map(|ty| ty.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            let Some(name) = dir.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(entry) = self
                .read_entry(&name, &running, cids.get(&name).cloned())
                .await
            {
                entries.push(entry);
            }
        }
        entries
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        let name = id.strip_prefix("capsule:").unwrap_or(id);
        let running = self.running_schemes().await;
        let cid = self.catalog_cids().await.get(name).cloned();
        self.read_entry(name, &running, cid).await
    }
}

pub struct AggregateInspectSource {
    sources: Vec<Arc<dyn InspectSource>>,
}

impl AggregateInspectSource {
    pub fn new(sources: Vec<Arc<dyn InspectSource>>) -> Self {
        Self { sources }
    }
}

#[async_trait]
impl InspectSource for AggregateInspectSource {
    async fn inspect_list(&self) -> Vec<InspectEntry> {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for source in &self.sources {
            for entry in source.inspect_list().await {
                if seen.insert(entry.id.clone()) {
                    entries.push(entry);
                }
            }
        }
        entries
    }

    async fn inspect_get(&self, id: &str) -> Option<InspectEntry> {
        for source in &self.sources {
            if let Some(entry) = source.inspect_get(id).await {
                return Some(entry);
            }
        }
        None
    }
}
