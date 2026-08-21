use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Local,
    Tofu,
}

impl Default for Backend {
    fn default() -> Self {
        Self::Local
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Postgres,
    Redis,
    Bucket,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub version: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub project: String,
    #[serde(default)]
    pub backend: Backend,
    #[serde(default)]
    pub resources: Vec<Resource>,
}

impl Project {
    pub fn load(path: &std::path::Path) -> Result<Self, crate::Error> {
        let raw = std::fs::read_to_string(path)?;
        let spec: Self = serde_yaml::from_str(&raw)?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.project.trim().is_empty() {
            return Err(crate::Error::Spec("project name is empty".into()));
        }
        let mut names = std::collections::BTreeSet::new();
        for r in &self.resources {
            if r.name.trim().is_empty() {
                return Err(crate::Error::Spec("resource name is empty".into()));
            }
            if !names.insert(r.name.clone()) {
                return Err(crate::Error::Spec(format!("duplicate resource {}", r.name)));
            }
        }
        Ok(())
    }

    pub fn resource(&self, name: &str) -> Option<&Resource> {
        self.resources.iter().find(|r| r.name == name)
    }
}

impl Resource {
    pub fn image(&self) -> String {
        match self.kind {
            Kind::Postgres => format!("postgres:{}", self.version.as_deref().unwrap_or("16")),
            Kind::Redis => format!("redis:{}", self.version.as_deref().unwrap_or("7")),
            Kind::Bucket => format!("minio/minio:{}", self.version.as_deref().unwrap_or("latest")),
        }
    }

    pub fn default_port(&self) -> u16 {
        self.port.unwrap_or(match self.kind {
            Kind::Postgres => 5432,
            Kind::Redis => 6379,
            Kind::Bucket => 9000,
        })
    }
}
