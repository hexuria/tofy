use std::cell::Cell;
use std::marker::PhantomData;
use std::path::PathBuf;

use tofy_spec::{Backend, Bind, Export, Kind, Project, Resource, Size};

thread_local! {
    static OPEN_STACK: Cell<bool> = const { Cell::new(false) };
}

fn mark_stack_open() {
    OPEN_STACK.with(|c| c.set(true));
}

fn mark_stack_closed() {
    OPEN_STACK.with(|c| c.set(false));
}

pub(crate) fn stack_left_open() -> bool {
    OPEN_STACK.with(|c| c.get())
}

/// Resource declaration is open for setters. Adding to a stack consumes it.
pub enum Open {}

/// `stack(name)` before any resource is added. No plan/apply/output/run.
pub enum Empty {}

/// Stack has at least one resource. `add`, `plan`, `apply` only.
pub enum NonEmpty {}

/// Engine has applied. `output` / `run` only — no `add`, no second `apply`.
pub enum Applied {}

/// Kind-specific declarations that can be moved into a stack.
pub trait ResourceDecl: Into<Resource> {}

impl ResourceDecl for Postgres<Open> {}
impl ResourceDecl for Mysql<Open> {}
impl ResourceDecl for Redis<Open> {}
impl ResourceDecl for Bucket<Open> {}
impl ResourceDecl for Secret<Open> {}

/// Postgres resource declaration. Not a database client.
/// `Postgres<Open>` has version/port/size/bind/replicas/export.
#[derive(Debug, Clone)]
pub struct Postgres<S> {
    name: String,
    version: Option<String>,
    port: Option<u16>,
    size: Size,
    bind: Bind,
    replicas: u32,
    exports: Vec<Export>,
    _state: PhantomData<S>,
}

impl Postgres<Open> {
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

    /// Also inject the host URI under `env_name` (`tofy run` / outputs).
    pub fn export(mut self, env_name: impl Into<String>) -> Self {
        self.exports.push(Export::new(env_name));
        self
    }

    /// Also inject a named output key under `env_name`.
    pub fn export_key(mut self, env_name: impl Into<String>, key: impl Into<String>) -> Self {
        self.exports.push(Export::with_key(env_name, key));
        self
    }
}

impl From<Postgres<Open>> for Resource {
    fn from(p: Postgres<Open>) -> Self {
        Resource {
            name: p.name,
            kind: Kind::Postgres,
            version: p.version,
            port: p.port,
            size: p.size,
            bind: p.bind,
            replicas: p.replicas,
            exports: p.exports,
        }
    }
}

/// Mysql resource declaration. Not a database client.
/// `Mysql<Open>` has version/port/size/bind/replicas/export.
#[derive(Debug, Clone)]
pub struct Mysql<S> {
    name: String,
    version: Option<String>,
    port: Option<u16>,
    size: Size,
    bind: Bind,
    replicas: u32,
    exports: Vec<Export>,
    _state: PhantomData<S>,
}

impl Mysql<Open> {
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

    /// Also inject the host URI under `env_name` (`tofy run` / outputs).
    pub fn export(mut self, env_name: impl Into<String>) -> Self {
        self.exports.push(Export::new(env_name));
        self
    }

    /// Also inject a named output key under `env_name`.
    pub fn export_key(mut self, env_name: impl Into<String>, key: impl Into<String>) -> Self {
        self.exports.push(Export::with_key(env_name, key));
        self
    }
}

impl From<Mysql<Open>> for Resource {
    fn from(p: Mysql<Open>) -> Self {
        Resource {
            name: p.name,
            kind: Kind::Mysql,
            version: p.version,
            port: p.port,
            size: p.size,
            bind: p.bind,
            replicas: p.replicas,
            exports: p.exports,
        }
    }
}

/// Redis resource declaration. Not a live client.
/// `Redis<Open>` has version/port/size/bind/replicas/export.
#[derive(Debug, Clone)]
pub struct Redis<S> {
    name: String,
    version: Option<String>,
    port: Option<u16>,
    size: Size,
    bind: Bind,
    replicas: u32,
    exports: Vec<Export>,
    _state: PhantomData<S>,
}

impl Redis<Open> {
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

    /// Also inject the host URI under `env_name` (`tofy run` / outputs).
    pub fn export(mut self, env_name: impl Into<String>) -> Self {
        self.exports.push(Export::new(env_name));
        self
    }

    /// Also inject a named output key under `env_name`.
    pub fn export_key(mut self, env_name: impl Into<String>, key: impl Into<String>) -> Self {
        self.exports.push(Export::with_key(env_name, key));
        self
    }
}

impl From<Redis<Open>> for Resource {
    fn from(r: Redis<Open>) -> Self {
        Resource {
            name: r.name,
            kind: Kind::Redis,
            version: r.version,
            port: r.port,
            size: r.size,
            bind: r.bind,
            replicas: r.replicas,
            exports: r.exports,
        }
    }
}

/// Object-storage bucket declaration. Not an SDK client.
/// `Bucket<Open>` has version/port/size/bind/export. There is no `.replicas()`.
#[derive(Debug, Clone)]
pub struct Bucket<S> {
    name: String,
    version: Option<String>,
    port: Option<u16>,
    size: Size,
    bind: Bind,
    exports: Vec<Export>,
    _state: PhantomData<S>,
}

impl Bucket<Open> {
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

    /// Also inject the object-store endpoint under `env_name`.
    pub fn export(mut self, env_name: impl Into<String>) -> Self {
        self.exports.push(Export::new(env_name));
        self
    }

    /// Also inject a named output key under `env_name`.
    pub fn export_key(mut self, env_name: impl Into<String>, key: impl Into<String>) -> Self {
        self.exports.push(Export::with_key(env_name, key));
        self
    }
}

impl From<Bucket<Open>> for Resource {
    fn from(b: Bucket<Open>) -> Self {
        Resource {
            name: b.name,
            kind: Kind::Bucket,
            version: b.version,
            port: b.port,
            size: b.size,
            bind: b.bind,
            replicas: 1,
            exports: b.exports,
        }
    }
}

/// Generated high-entropy value, persisted in state. Not a container.
/// `Secret<Open>` has export only. There is no `.replicas()`, `.port()`, or `.version()`.
#[derive(Debug, Clone)]
pub struct Secret<S> {
    name: String,
    exports: Vec<Export>,
    _state: PhantomData<S>,
}

impl Secret<Open> {
    /// Also inject the generated value under `env_name`.
    pub fn export(mut self, env_name: impl Into<String>) -> Self {
        self.exports.push(Export::new(env_name));
        self
    }

    /// Also inject a named output key under `env_name` (secret default is `value`).
    pub fn export_key(mut self, env_name: impl Into<String>, key: impl Into<String>) -> Self {
        self.exports.push(Export::with_key(env_name, key));
        self
    }
}

impl From<Secret<Open>> for Resource {
    fn from(s: Secret<Open>) -> Self {
        Resource {
            name: s.name,
            kind: Kind::Secret,
            version: None,
            port: None,
            size: Size::Small,
            bind: Bind::Localhost,
            replicas: 1,
            exports: s.exports,
        }
    }
}

pub struct Stack<S> {
    project: Project,
    _state: PhantomData<S>,
}

impl Stack<Empty> {
    /// Select the apply engine. Default is [`Backend::Local`] (Docker).
    /// Illegal after `add` — this method exists only on [`Stack<Empty>`].
    pub fn backend(mut self, backend: Backend) -> Self {
        self.project.backend = backend;
        self
    }

    /// `stack("demo").backend(Backend::Tofu)`.
    pub fn tofu(self) -> Self {
        self.backend(Backend::Tofu)
    }

    /// `stack("demoaws").backend(Backend::Aws)`.
    pub fn aws(self) -> Self {
        self.backend(Backend::Aws)
    }

    pub fn add(mut self, resource: impl ResourceDecl) -> Stack<NonEmpty> {
        self.project.resources.push(resource.into());
        Stack {
            project: self.project,
            _state: PhantomData,
        }
    }
}

impl Stack<NonEmpty> {
    pub fn add(mut self, resource: impl ResourceDecl) -> Stack<NonEmpty> {
        self.project.resources.push(resource.into());
        self
    }

    /// Print the plan. Local: spec vs state and live Docker.
    /// Tofu / Aws: OpenTofu engine plan against `.tofy/main.tf.json`.
    pub fn plan(self) {
        mark_stack_closed();
        let root = workdir();
        match crate::engine::plan_text(&root, &self.project) {
            Ok(text) => print!("{text}"),
            Err(e) => {
                eprintln!("tofy: {e}");
                std::process::exit(e.exit_code());
            }
        }
    }

    /// Apply the stack. Returns [`Stack<Applied>`] only after a real apply.
    ///
    /// `cargo run -p infra` (no verb) applies. `cargo run -- plan` (and
    /// destroy / output / run / emit) run that verb and exit without
    /// returning Applied.
    pub fn apply(self) -> Stack<Applied> {
        mark_stack_closed();
        match crate::cli::run_declared(self.project.clone()) {
            Ok(crate::cli::DeclaredOutcome::Applied) => Stack {
                project: self.project,
                _state: PhantomData,
            },
            Ok(crate::cli::DeclaredOutcome::Finished) => std::process::exit(0),
            Err(e) => {
                eprintln!("tofy: {e}");
                std::process::exit(e.exit_code());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn into_project(self) -> Project {
        mark_stack_closed();
        self.project
    }
}

impl Stack<Applied> {
    /// Host URI for a resource (`TOFY_<NAME>_URI`) from `.tofy/outputs.json`.
    /// Does not open a client. Opt-in `tofy-pg` can turn this into a `PgPool`.
    pub fn uri(&self, name: &str) -> crate::Result<String> {
        let root = workdir();
        let map = crate::outputs::load(&root)?;
        let key = tofy_spec::env_var(name, "uri");
        map.get(&key)
            .cloned()
            .ok_or_else(|| crate::error::Error::Engine(format!("no URI for resource {name}")))
    }

    /// Print non-secret outputs (`tofy output`).
    pub fn output(self) {
        let root = workdir();
        match crate::outputs::load(&root) {
            Ok(map) => print!("{}", crate::outputs::format_public(&map)),
            Err(e) => {
                eprintln!("tofy: {e}");
                std::process::exit(e.exit_code());
            }
        }
    }

    /// Inject outputs and exec. Equivalent to `tofy run -- <cmd>`.
    pub fn run<I, A>(self, args: I)
    where
        I: IntoIterator<Item = A>,
        A: Into<String>,
    {
        let root = workdir();
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        if let Err(e) = crate::cli::run_command(&root, &args) {
            eprintln!("tofy: {e}");
            std::process::exit(e.exit_code());
        }
    }
}

fn workdir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--dir" {
            if let Some(d) = args.next() {
                return PathBuf::from(d);
            }
        } else if let Some(d) = a.strip_prefix("--dir=") {
            return PathBuf::from(d);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn postgres(name: impl Into<String>) -> Postgres<Open> {
    Postgres {
        name: name.into(),
        version: None,
        port: None,
        size: Size::Small,
        bind: Bind::Localhost,
        replicas: 1,
        exports: Vec::new(),
        _state: PhantomData,
    }
}

pub fn mysql(name: impl Into<String>) -> Mysql<Open> {
    Mysql {
        name: name.into(),
        version: None,
        port: None,
        size: Size::Small,
        bind: Bind::Localhost,
        replicas: 1,
        exports: Vec::new(),
        _state: PhantomData,
    }
}

pub fn redis(name: impl Into<String>) -> Redis<Open> {
    Redis {
        name: name.into(),
        version: None,
        port: None,
        size: Size::Small,
        bind: Bind::Localhost,
        replicas: 1,
        exports: Vec::new(),
        _state: PhantomData,
    }
}

pub fn bucket(name: impl Into<String>) -> Bucket<Open> {
    Bucket {
        name: name.into(),
        version: None,
        port: None,
        size: Size::Small,
        bind: Bind::Localhost,
        exports: Vec::new(),
        _state: PhantomData,
    }
}

pub fn secret(name: impl Into<String>) -> Secret<Open> {
    Secret {
        name: name.into(),
        exports: Vec::new(),
        _state: PhantomData,
    }
}

pub fn stack(name: impl Into<String>) -> Stack<Empty> {
    mark_stack_open();
    Stack {
        project: Project::new(name),
        _state: PhantomData,
    }
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
        let cache = redis("cache").size(Size::Medium);
        let files = bucket("uploads");
        let sql = mysql("appmysql").port(3307).version("8");
        let project = stack("demo")
            .add(db)
            .add(cache)
            .add(files)
            .add(sql)
            .into_project();
        assert_eq!(project.project, "demo");
        assert_eq!(project.backend, Backend::Local);
        assert_eq!(project.docker_network(), "tofy-demo");
        assert_eq!(project.resources.len(), 4);
        assert_eq!(project.resources[0].name, "appdb");
        assert_eq!(project.resources[0].kind, Kind::Postgres);
        assert_eq!(project.resources[0].port, Some(5433));
        assert_eq!(project.resources[0].size, Size::Small);
        assert_eq!(project.resources[0].replicas, 1);
        assert_eq!(project.resources[1].kind, Kind::Redis);
        assert_eq!(project.resources[1].replicas, 1);
        assert_eq!(project.resources[1].size, Size::Medium);
        assert_eq!(project.resources[2].kind, Kind::Bucket);
        assert_eq!(project.resources[2].replicas, 1);
        assert_eq!(project.resources[3].kind, Kind::Mysql);
        assert_eq!(project.resources[3].port, Some(3307));
        assert_eq!(project.resources[3].version.as_deref(), Some("8"));
    }

    #[test]
    fn replicas_on_engines() {
        assert_eq!(Resource::from(postgres("appdb").replicas(2)).replicas, 2);
        assert_eq!(Resource::from(mysql("appmysql").replicas(3)).replicas, 3);
        assert_eq!(Resource::from(redis("cache").replicas(2)).replicas, 2);
    }

    #[test]
    fn backend_selector_on_empty() {
        let via_enum = stack("demo")
            .backend(Backend::Tofu)
            .add(postgres("appdb"))
            .into_project();
        assert_eq!(via_enum.backend, Backend::Tofu);
        assert_eq!(via_enum.resources[0].name, "appdb");
        let via_tofu = stack("demo").tofu().add(redis("cache")).into_project();
        assert_eq!(via_tofu.backend, Backend::Tofu);
        let local = stack("demo")
            .backend(Backend::Local)
            .add(bucket("uploads"))
            .into_project();
        assert_eq!(local.backend, Backend::Local);
        let via_aws = stack("demoaws")
            .backend(Backend::Aws)
            .add(postgres("appdb"))
            .add(redis("cache"))
            .add(bucket("uploads"))
            .into_project();
        assert_eq!(via_aws.backend, Backend::Aws);
        assert_eq!(via_aws.resources.len(), 3);
        let via_aws_fn = stack("demoaws").aws().add(bucket("uploads")).into_project();
        assert_eq!(via_aws_fn.backend, Backend::Aws);
    }

    #[test]
    fn export_and_secret_on_open_builders() {
        let db = postgres("appdb")
            .version("18")
            .port(5452)
            .export("OAG_DATABASE__URL");
        let cache = redis("cache")
            .version("8")
            .port(6399)
            .export("OAG_REDIS__URL");
        let files = bucket("uploads").export("APP_S3_ENDPOINT");
        let sql = mysql("appmysql").export_key("APP_MYSQL_PASSWORD", "password");
        let sign = secret("signing").export("OAG_SECURITY__SIGNING_SECRET");
        let kek = secret("kek").export("OAG_SECURITY__CREDENTIAL_KEK");
        let project = stack("oag")
            .add(db)
            .add(cache)
            .add(files)
            .add(sql)
            .add(sign)
            .add(kek)
            .into_project();
        assert_eq!(project.project, "oag");
        assert_eq!(project.resources[0].exports[0].env, "OAG_DATABASE__URL");
        assert_eq!(
            project.resources[0].exports[0].output_key(Kind::Postgres),
            "uri"
        );
        assert_eq!(project.resources[1].exports[0].env, "OAG_REDIS__URL");
        assert_eq!(
            project.resources[2].exports[0].output_key(Kind::Bucket),
            "endpoint"
        );
        assert_eq!(
            project.resources[3].exports[0].key.as_deref(),
            Some("password")
        );
        assert_eq!(project.resources[4].kind, Kind::Secret);
        assert_eq!(project.resources[4].name, "signing");
        assert_eq!(
            project.resources[5].exports[0].env,
            "OAG_SECURITY__CREDENTIAL_KEK"
        );
        project.validate().unwrap();
    }
}
