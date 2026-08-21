use std::cell::RefCell;

use tofy_spec::{Bind, Kind, Project, Resource, Size};

thread_local! {
    static DECLARED: RefCell<Option<Project>> = const { RefCell::new(None) };
}

/// Postgres resource declaration. Not a database client.
/// Local postgres has no HA; there is no `.replicas()` on this type.
#[derive(Debug, Clone)]
pub struct Postgres {
    name: String,
    version: Option<String>,
    port: Option<u16>,
    size: Size,
    bind: Bind,
}

impl Postgres {
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn bind(mut self, bind: Bind) -> Self {
        self.bind = bind;
        self
    }
}

impl From<Postgres> for Resource {
    fn from(p: Postgres) -> Self {
        Resource {
            name: p.name,
            kind: Kind::Postgres,
            version: p.version,
            port: p.port,
            size: p.size,
            bind: p.bind,
            replicas: 1,
        }
    }
}

/// Redis resource declaration. Not a live client.
#[derive(Debug, Clone)]
pub struct Redis {
    name: String,
    version: Option<String>,
    port: Option<u16>,
    size: Size,
    bind: Bind,
    replicas: u32,
}

impl Redis {
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn bind(mut self, bind: Bind) -> Self {
        self.bind = bind;
        self
    }

    pub fn replicas(mut self, n: u32) -> Self {
        self.replicas = n;
        self
    }
}

impl From<Redis> for Resource {
    fn from(r: Redis) -> Self {
        Resource {
            name: r.name,
            kind: Kind::Redis,
            version: r.version,
            port: r.port,
            size: r.size,
            bind: r.bind,
            replicas: r.replicas,
        }
    }
}

/// Object-storage bucket declaration. Not an SDK client.
#[derive(Debug, Clone)]
pub struct Bucket {
    name: String,
    version: Option<String>,
    port: Option<u16>,
    size: Size,
    bind: Bind,
    replicas: u32,
}

impl Bucket {
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn bind(mut self, bind: Bind) -> Self {
        self.bind = bind;
        self
    }

    pub fn replicas(mut self, n: u32) -> Self {
        self.replicas = n;
        self
    }
}

impl From<Bucket> for Resource {
    fn from(b: Bucket) -> Self {
        Resource {
            name: b.name,
            kind: Kind::Bucket,
            version: b.version,
            port: b.port,
            size: b.size,
            bind: b.bind,
            replicas: b.replicas,
        }
    }
}

pub struct Stack;

impl Stack {
    pub fn add(self, resource: impl Into<Resource>) -> Self {
        DECLARED.with(|slot| {
            if let Some(project) = slot.borrow_mut().as_mut() {
                project.resources.push(resource.into());
            }
        });
        self
    }
}

pub fn postgres(name: impl Into<String>) -> Postgres {
    Postgres {
        name: name.into(),
        version: None,
        port: None,
        size: Size::Small,
        bind: Bind::Localhost,
    }
}

pub fn redis(name: impl Into<String>) -> Redis {
    Redis {
        name: name.into(),
        version: None,
        port: None,
        size: Size::Small,
        bind: Bind::Localhost,
        replicas: 1,
    }
}

pub fn bucket(name: impl Into<String>) -> Bucket {
    Bucket {
        name: name.into(),
        version: None,
        port: None,
        size: Size::Small,
        bind: Bind::Localhost,
        replicas: 1,
    }
}

pub fn stack(name: impl Into<String>) -> Stack {
    DECLARED.with(|slot| {
        *slot.borrow_mut() = Some(Project::new(name));
    });
    Stack
}

pub fn take_project() -> Option<Project> {
    DECLARED.with(|slot| slot.borrow_mut().take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_emit_spec() {
        let db = postgres("appdb")
            .version("16")
            .port(5433)
            .size(Size::Small)
            .bind(Bind::Localhost);
        let cache = redis("cache").replicas(2).size(Size::Medium);
        let files = bucket("uploads");
        stack("demo").add(db).add(cache).add(files);
        let project = take_project().unwrap();
        assert_eq!(project.project, "demo");
        assert_eq!(project.docker_network(), "tofy-demo");
        assert_eq!(project.resources.len(), 3);
        assert_eq!(project.resources[0].name, "appdb");
        assert_eq!(project.resources[0].kind, Kind::Postgres);
        assert_eq!(project.resources[0].port, Some(5433));
        assert_eq!(project.resources[0].size, Size::Small);
        assert_eq!(project.resources[0].replicas, 1);
        assert_eq!(project.resources[1].kind, Kind::Redis);
        assert_eq!(project.resources[1].replicas, 2);
        assert_eq!(project.resources[1].size, Size::Medium);
        assert_eq!(project.resources[2].kind, Kind::Bucket);
    }
}
