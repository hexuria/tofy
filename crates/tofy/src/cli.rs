use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};
use tofy_spec::Project;

use crate::engine;
use crate::error::{Error, Result};
use crate::outputs;

#[derive(Parser, Debug)]
#[command(
    name = "tofy",
    version,
    about = "Rust control language for infrastructure. Plan and apply like OpenTofu."
)]
pub struct Cli {
    /// Directory that holds the stack (or .tofy state)
    #[arg(long, global = true, default_value = ".")]
    pub dir: PathBuf,

    /// Already-emitted spec JSON (or a YAML import). Skips compiling a Rust stack.
    #[arg(long, global = true)]
    pub spec: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Cmd {
    /// Diff the declared stack against .tofy/state.json
    Plan,
    /// Create, update, and delete resources
    Apply,
    /// Tear down containers and clear state
    Destroy,
    /// Print outputs (secrets omitted unless --json)
    Output {
        /// Dump every key, including secrets, as JSON
        #[arg(long)]
        json: bool,
    },
    /// Inject outputs as env vars and exec a command
    Run {
        #[arg(last = true, required = true)]
        args: Vec<String>,
    },
    /// Write spec JSON and OpenTofu JSON without applying
    Emit,
}

impl Cmd {
    fn or_apply(cmd: Option<Self>) -> Self {
        cmd.unwrap_or(Cmd::Apply)
    }
}

pub fn run() -> Result<()> {
    run_inner(None)
}

pub fn run_with_project(project: Project) -> Result<()> {
    run_inner(Some(project))
}

fn run_inner(declared: Option<Project>) -> Result<()> {
    let cli = Cli::parse();
    let root = std::fs::canonicalize(&cli.dir).unwrap_or_else(|_| cli.dir.clone());
    let cmd = Cmd::or_apply(cli.cmd.clone());

    if declared.is_none() && cli.spec.is_none() && is_declaration_crate(&root) {
        return forward_to_declaration(&root, &cli, &cmd);
    }

    match cmd {
        Cmd::Destroy => {
            print!("{}", engine::destroy(&root)?);
            Ok(())
        }
        Cmd::Output { json } => {
            let map = outputs::load(&root)?;
            if json {
                print!("{}", serde_json::to_string_pretty(&map)?);
                if !map.is_empty() {
                    println!();
                }
            } else {
                print!("{}", outputs::format_public(&map));
            }
            Ok(())
        }
        Cmd::Run { args } => run_command(&root, &args),
        Cmd::Plan | Cmd::Apply | Cmd::Emit => {
            let spec = load_spec(&root, cli.spec.as_ref(), declared)?;
            match cmd {
                Cmd::Plan => {
                    print!("{}", engine::plan_text(&root, &spec)?);
                    Ok(())
                }
                Cmd::Apply => {
                    print!("{}", engine::apply(&root, &spec)?);
                    Ok(())
                }
                Cmd::Emit => {
                    print!("{}", engine::emit(&root, &spec)?);
                    Ok(())
                }
                _ => unreachable!(),
            }
        }
    }
}

fn load_spec(
    root: &Path,
    spec_flag: Option<&PathBuf>,
    declared: Option<Project>,
) -> Result<Project> {
    if let Some(path) = spec_flag {
        return load_spec_file(path);
    }
    if let Some(project) = declared {
        project.validate()?;
        return Ok(project);
    }
    let yaml = root.join("tofy.yaml");
    if yaml.exists() {
        return load_spec_file(&yaml);
    }
    let json = root.join(".tofy").join("spec.json");
    if json.exists() {
        return Ok(Project::load_json(&json)?);
    }
    Err(Error::Spec(tofy_spec::SpecError::Validation(format!(
        "no stack declaration, spec JSON, or tofy.yaml in {}",
        root.display()
    ))))
}

pub fn load_spec_file(path: &Path) -> Result<Project> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "yaml" || ext == "yml" {
        let raw = std::fs::read_to_string(path)?;
        let spec: Project = serde_yaml::from_str(&raw)?;
        spec.validate()?;
        Ok(spec)
    } else {
        Ok(Project::load_json(path)?)
    }
}

fn is_declaration_crate(dir: &Path) -> bool {
    let cargo_path = dir.join("Cargo.toml");
    if !cargo_path.exists() || !dir.join("src/main.rs").exists() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(&cargo_path) else {
        return false;
    };
    let is_tofy_engine = text.lines().any(|l| l.trim() == "name = \"tofy\"");
    if is_tofy_engine {
        return false;
    }
    text.contains("[package]") && text.contains("tofy")
}

fn forward_to_declaration(dir: &Path, cli: &Cli, cmd: &Cmd) -> Result<()> {
    let mut args: Vec<String> = vec!["--dir".into(), dir.display().to_string()];
    if let Some(spec) = &cli.spec {
        args.push("--spec".into());
        args.push(spec.display().to_string());
    }
    match cmd {
        Cmd::Plan => args.push("plan".into()),
        Cmd::Apply => args.push("apply".into()),
        Cmd::Destroy => args.push("destroy".into()),
        Cmd::Output { json } => {
            args.push("output".into());
            if *json {
                args.push("--json".into());
            }
        }
        Cmd::Run { args: rest } => {
            args.push("run".into());
            args.push("--".into());
            args.extend(rest.iter().cloned());
        }
        Cmd::Emit => args.push("emit".into()),
    }

    let status = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--")
        .args(&args)
        .current_dir(dir)
        .status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

pub(crate) fn run_command(root: &Path, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(Error::Usage("tofy run -- <command>".into()));
    }
    let env = outputs::load(root)?;
    let mut cmd = Command::new(&args[0]);
    if args.len() > 1 {
        cmd.args(&args[1..]);
    }
    cmd.envs(env);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        return Err(Error::Engine(format!("exec failed: {err}")));
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status()?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn yaml_imports_to_same_ir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tofy.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(
            b"project: demo\nbackend: local\nresources:\n  - name: appdb\n    type: postgres\n    version: \"16\"\n    port: 5433\n",
        )
        .unwrap();
        let spec = load_spec_file(&path).unwrap();
        assert_eq!(spec.project, "demo");
        assert_eq!(spec.resources.len(), 1);
        assert_eq!(spec.resources[0].name, "appdb");
        let json = spec.to_json_pretty().unwrap();
        let again = Project::from_json_str(&json).unwrap();
        assert_eq!(spec, again);
    }
}
