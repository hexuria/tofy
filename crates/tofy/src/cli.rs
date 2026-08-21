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

    /// Already-emitted spec JSON. Skips compiling a Rust stack.
    #[arg(long, global = true)]
    pub spec: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Cmd {
    /// Local: live Docker vs state. Tofu: OpenTofu engine plan.
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
    /// Write spec JSON without applying
    Emit,
}

impl Cmd {
    fn or_apply(cmd: Option<Self>) -> Self {
        cmd.unwrap_or(Cmd::Apply)
    }
}

/// What `Stack::apply` did. Only [`DeclaredOutcome::Applied`] may become `Stack<Applied>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredOutcome {
    Applied,
    Finished,
}

pub fn run() -> Result<()> {
    dispatch(None).map(|_| ())
}

pub fn run_declared(project: Project) -> Result<DeclaredOutcome> {
    dispatch(Some(project))
}

fn dispatch(declared: Option<Project>) -> Result<DeclaredOutcome> {
    let cli = Cli::parse();
    let root = std::fs::canonicalize(&cli.dir).unwrap_or_else(|_| cli.dir.clone());
    let cmd = Cmd::or_apply(cli.cmd.clone());

    if declared.is_none() && cli.spec.is_none() && is_declaration_crate(&root) {
        forward_to_declaration(&root, &cli, &cmd)?;
        return Ok(DeclaredOutcome::Finished);
    }

    match cmd {
        Cmd::Destroy => {
            print!("{}", engine::destroy(&root)?);
            Ok(DeclaredOutcome::Finished)
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
            Ok(DeclaredOutcome::Finished)
        }
        Cmd::Run { args } => {
            run_command(&root, &args)?;
            Ok(DeclaredOutcome::Finished)
        }
        Cmd::Plan => {
            let spec = load_spec(&root, cli.spec.as_ref(), declared)?;
            print!("{}", engine::plan_text(&root, &spec)?);
            Ok(DeclaredOutcome::Finished)
        }
        Cmd::Emit => {
            let spec = load_spec(&root, cli.spec.as_ref(), declared)?;
            print!("{}", engine::emit(&root, &spec)?);
            Ok(DeclaredOutcome::Finished)
        }
        Cmd::Apply => {
            let spec = load_spec(&root, cli.spec.as_ref(), declared)?;
            print!("{}", engine::apply(&root, &spec)?);
            Ok(DeclaredOutcome::Applied)
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
    let json = root.join(".tofy").join("spec.json");
    if json.exists() {
        return Ok(Project::load_json(&json)?);
    }
    Err(Error::Spec(tofy_spec::SpecError::Validation(format!(
        "no stack declaration or spec JSON in {}",
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
        return Err(Error::Usage(
            "spec must be JSON IR (`--spec spec.json`); yaml is not a write path".into(),
        ));
    }
    Ok(Project::load_json(path)?)
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

    #[test]
    fn spec_json_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spec.json");
        std::fs::write(
            &path,
            r#"{"project":"demo","resources":[{"name":"appdb","type":"postgres"}]}"#,
        )
        .unwrap();
        let spec = load_spec_file(&path).unwrap();
        assert_eq!(spec.project, "demo");
        assert_eq!(spec.resources[0].name, "appdb");
    }

    #[test]
    fn spec_flag_rejects_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tofy.yaml");
        std::fs::write(&path, "project: demo\nresources: []\n").unwrap();
        let err = load_spec_file(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("JSON"), "{msg}");
        assert!(!msg.contains("add tofy.yaml"), "{msg}");
    }

    #[test]
    fn does_not_autoload_tofy_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tofy.yaml"),
            "project: demo\nresources: []\n",
        )
        .unwrap();
        let err = load_spec(dir.path(), None, None).unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("tofy.yaml"), "{msg}");
        assert!(
            msg.contains("spec JSON") || msg.contains("stack declaration"),
            "{msg}"
        );
    }
}
