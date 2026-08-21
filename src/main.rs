mod emit;
mod engine;
mod spec;
mod state;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use spec::Project;
use state::State;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("spec: {0}")]
    Spec(String),
    #[error("{0}")]
    Engine(String),
}

#[derive(Parser)]
#[command(name = "tofy", version, about = "Infrastructure from code, applied like OpenTofu")]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a starter tofy.yaml
    Init,
    /// Diff spec against state
    Plan,
    /// Write artifacts and apply locally if Docker is present
    Apply,
    /// Tear down local resources
    Destroy,
    /// Print resource outputs
    Output {
        #[arg(long)]
        json: bool,
    },
    /// Write OpenTofu JSON and compose only
    Emit,
}

fn spec_path(root: &Path) -> PathBuf {
    root.join("tofy.yaml")
}

fn load_spec(root: &Path) -> Result<Project, Error> {
    let p = spec_path(root);
    if !p.exists() {
        return Err(Error::Spec(format!(
            "no tofy.yaml in {}. Run `tofy init`.",
            root.display()
        )));
    }
    Project::load(&p)
}

fn init(root: &Path) -> Result<String, Error> {
    let p = spec_path(root);
    if p.exists() {
        return Err(Error::Spec("tofy.yaml already exists".into()));
    }
    std::fs::create_dir_all(root)?;
    std::fs::write(
        &p,
        r#"# Declare what the app needs. tofy plans it, emits OpenTofu, and
# applies locally with Docker when it can.
project: demo
backend: local
resources:
  - name: appdb
    type: postgres
    version: "16"
    port: 5433
"#,
    )?;
    Ok(format!("Wrote {}\n", p.display()))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("tofy: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    let root = cli.dir.canonicalize().unwrap_or(cli.dir);
    let text = match cli.cmd {
        Cmd::Init => init(&root)?,
        Cmd::Plan => {
            let spec = load_spec(&root)?;
            let current = State::load(&root)?;
            engine::format_actions(&state::plan(&spec, &current))
        }
        Cmd::Apply => {
            let spec = load_spec(&root)?;
            engine::apply(&root, &spec)?
        }
        Cmd::Destroy => engine::destroy(&root)?,
        Cmd::Output { json } => {
            let outs = root.join(".tofy").join("outputs.json");
            if !outs.exists() {
                return Err(Error::Engine("no outputs. run `tofy apply` first".into()));
            }
            let raw = std::fs::read_to_string(outs)?;
            if json {
                raw
            } else {
                let v: serde_json::Value = serde_json::from_str(&raw)?;
                let mut s = String::new();
                if let Some(map) = v.as_object() {
                    for (name, vals) in map {
                        s.push_str(&format!("{name}\n"));
                        if let Some(inner) = vals.as_object() {
                            for (k, val) in inner {
                                s.push_str(&format!("  {k} = {}\n", val.as_str().unwrap_or("")));
                            }
                        }
                    }
                }
                s
            }
        }
        Cmd::Emit => {
            let spec = load_spec(&root)?;
            emit::write_artifacts(&root, &spec)?;
            "Wrote .tofy/main.tf.json, .tofy/outputs.json, docker-compose.yml\n".into()
        }
    };
    print!("{text}");
    Ok(())
}
