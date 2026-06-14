use std::{
    env,
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

use tinyjson::JsonValue;

use crate::{Result, cli, kernel::KernelBuild, process};

pub struct Package {
    pub source: PathBuf,
    id: String,
    pub crate_name: String,
}

pub struct Cargo {
    executable: OsString,
}

impl Cargo {
    pub fn from_env() -> Self {
        Self {
            executable: env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        }
    }

    pub fn package(&self, args: &[String]) -> Result<Package> {
        let manifest_path = cli::path_option(args, "--manifest-path");
        let mut command = Command::new(&self.executable);
        command.args(["metadata", "--format-version", "1", "--no-deps"]);
        if let Some(path) = &manifest_path {
            command.arg("--manifest-path").arg(path);
        }

        let output = command.output()?;
        if !output.status.success() {
            return Err("cargo metadata failed".into());
        }

        let json = String::from_utf8(output.stdout)?;
        let parsed: JsonValue = json.parse()?;
        let packages = parsed["packages"]
            .get::<Vec<JsonValue>>()
            .ok_or("cargo metadata has no packages")?;
        let requested = cli::string_option(args, "--package", Some("-p"));
        let package = if let Some(requested) = requested {
            packages
                .iter()
                .find(|package| {
                    package["name"]
                        .get::<String>()
                        .is_some_and(|name| name == &requested)
                        || package["id"]
                            .get::<String>()
                            .is_some_and(|id| id == &requested)
                })
                .ok_or_else(|| format!("package `{requested}` was not found in Cargo metadata"))?
        } else if packages.len() == 1 {
            &packages[0]
        } else {
            return Err(
                "workspace contains multiple packages; select the module with `-p <package>`"
                    .into(),
            );
        };
        let targets = package["targets"]
            .get::<Vec<JsonValue>>()
            .ok_or("package has no targets")?;
        let target = targets
            .iter()
            .find(|target| {
                target["kind"].get::<Vec<JsonValue>>().is_some_and(|kinds| {
                    kinds
                        .iter()
                        .any(|kind| kind.get::<String>().is_some_and(|kind| kind == "rlib"))
                })
            })
            .ok_or("package must have an rlib target")?;

        Ok(Package {
            source: PathBuf::from(
                target["src_path"]
                    .get::<String>()
                    .ok_or("rlib target has no src_path")?,
            ),
            id: package["id"]
                .get::<String>()
                .ok_or("package has no id")?
                .clone(),
            crate_name: target["name"]
                .get::<String>()
                .ok_or("rlib target has no name")?
                .clone(),
        })
    }

    pub fn build(
        &self,
        kernel: &KernelBuild,
        args: &[String],
        package: &Package,
        module_name: &str,
    ) -> Result<PathBuf> {
        let mut command = self.command("build", kernel, args, module_name);
        command
            .arg("--message-format=json-render-diagnostics")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let printable = format!("{command:?}");
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or("failed to capture Cargo output")?;
        let mut rlib = None;

        for line in BufReader::new(stdout).lines() {
            let line = line?;
            let Ok(message) = line.parse::<JsonValue>() else {
                println!("{line}");
                continue;
            };

            match message["reason"].get::<String>().map(String::as_str) {
                Some("compiler-message") => {
                    if let Some(rendered) = message["message"]["rendered"].get::<String>() {
                        eprint!("{rendered}");
                    }
                }
                Some("compiler-artifact")
                    if message["package_id"].get::<String>() == Some(&package.id)
                        && message["target"]["name"].get::<String>()
                            == Some(&package.crate_name) =>
                {
                    if let Some(filenames) = message["filenames"].get::<Vec<JsonValue>>() {
                        rlib = filenames
                            .iter()
                            .filter_map(|filename| filename.get::<String>())
                            .map(PathBuf::from)
                            .find(|filename| filename.extension() == Some(OsStr::new("rlib")));
                    }
                }
                _ => {}
            }
        }

        let status = child.wait()?;
        if !status.success() {
            return Err(format!("command failed with {status}: {printable}").into());
        }

        rlib.ok_or_else(|| {
            format!(
                "Cargo did not report an rlib artifact for `{}`",
                package.crate_name
            )
            .into()
        })
    }

    pub fn expand(&self, kernel: &KernelBuild, args: &[String], module_name: &str) -> Result<()> {
        process::run(&mut self.command("expand", kernel, args, module_name))
    }

    fn command(
        &self,
        subcommand: &str,
        kernel: &KernelBuild,
        args: &[String],
        module_name: &str,
    ) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .arg(subcommand)
            .arg("-Zjson-target-spec")
            .arg("--target")
            .arg(kernel.rust_target())
            .args(args)
            .env("OBJTREE", &kernel.dir)
            .env("RUST_MODFILE", module_name)
            .env("NOK_KBUILD_DIR", &kernel.dir)
            .env("RUSTC_BOOTSTRAP", "1")
            .env(
                "CARGO_ENCODED_RUSTFLAGS",
                kernel.rust_flags().join("\u{1f}"),
            );
        command
    }
}
