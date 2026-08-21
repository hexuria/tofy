use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::spec::{Kind, Project};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Planned,
    Emitted,
    Applied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceState {
    pub kind: Kind,
    pub status: Status,
    pub image: String,
    pub port: u16,
    pub outputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct State {
    pub project: String,
    pub resources: BTreeMap<String, ResourceState>,
}

impl State {
    pub fn path(root: &Path) -> PathBuf {
        root.join(".tofy").join("state.json")
    }

    pub fn load(root: &Path) -> Result<Self, crate::Error> {
        let p = Self::path(root);
        if !p.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?)
    }

    pub fn save(&self, root: &Path) -> Result<(), crate::Error> {
        let dir = root.join(".tofy");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(Self::path(root), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn from_spec(spec: &Project) -> Self {
        let mut resources = BTreeMap::new();
        for r in &spec.resources {
            resources.insert(
                r.name.clone(),
                ResourceState {
                    kind: r.kind.clone(),
                    status: Status::Planned,
                    image: r.image(),
                    port: r.default_port(),
                    outputs: outputs_for(spec, r),
                },
            );
        }
        Self {
            project: spec.project.clone(),
            resources,
        }
    }
}

pub fn outputs_for(spec: &Project, r: &crate::spec::Resource) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let port = r.default_port();
    match r.kind {
        Kind::Postgres => {
            let user = "tofy";
            let pass = format!("tofy-{}-{}", spec.project, r.name);
            let db = r.name.replace('-', "_");
            out.insert(
                "uri".into(),
                format!("postgres://{user}:{pass}@127.0.0.1:{port}/{db}"),
            );
            out.insert("user".into(), user.into());
            out.insert("password".into(), pass);
            out.insert("database".into(), db);
            out.insert("port".into(), port.to_string());
        }
        Kind::Redis => {
            out.insert("uri".into(), format!("redis://127.0.0.1:{port}"));
            out.insert("port".into(), port.to_string());
        }
        Kind::Bucket => {
            out.insert("endpoint".into(), format!("http://127.0.0.1:{port}"));
            out.insert("access_key".into(), "tofy".into());
            out.insert("secret_key".into(), format!("tofy-{}-{}", spec.project, r.name));
            out.insert("bucket".into(), r.name.clone());
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Create { name: String, kind: Kind },
    Update { name: String, reason: String },
    Delete { name: String, kind: Kind },
    Noop { name: String },
}

pub fn plan(spec: &Project, current: &State) -> Vec<Action> {
    let desired = State::from_spec(spec);
    let mut actions = Vec::new();
    for (name, want) in &desired.resources {
        match current.resources.get(name) {
            None => actions.push(Action::Create {
                name: name.clone(),
                kind: want.kind.clone(),
            }),
            Some(have) if have.kind != want.kind || have.image != want.image || have.port != want.port => {
                actions.push(Action::Update {
                    name: name.clone(),
                    reason: "image, type, or port changed".into(),
                });
            }
            Some(_) => actions.push(Action::Noop { name: name.clone() }),
        }
    }
    for (name, have) in &current.resources {
        if !desired.resources.contains_key(name) {
            actions.push(Action::Delete {
                name: name.clone(),
                kind: have.kind.clone(),
            });
        }
    }
    actions
}
