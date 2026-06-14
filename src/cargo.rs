use std::{
    env,
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

use serde_json::Value;

use crate::{Result, cli, kernel::KernelBuild, process};

pub struct Package {
    pub source: PathBuf,
    id: String,
    pub crate_name: String,
    pub modfile: PathBuf,
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

        let parsed: Value = serde_json::from_slice(&output.stdout)?;
        let packages = parsed["packages"]
            .as_array()
            .ok_or("cargo metadata has no packages")?;
        let requested = cli::string_option(args, "--package", Some("-p"));
        let package = if let Some(requested) = requested {
            packages
                .iter()
                .find(|package| {
                    package["name"].as_str() == Some(&requested)
                        || package["id"].as_str() == Some(&requested)
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
            .as_array()
            .ok_or("package has no targets")?;
        let target = targets
            .iter()
            .find(|target| {
                target["kind"].as_array().is_some_and(|kinds| {
                    kinds
                        .iter()
                        .any(|kind| kind.as_str().is_some_and(|kind| kind == "rlib"))
                })
            })
            .ok_or("package must have an rlib target")?;

        let source = PathBuf::from(
            target["src_path"]
                .as_str()
                .ok_or("rlib target has no src_path")?,
        );
        let manifest = PathBuf::from(
            package["manifest_path"]
                .as_str()
                .ok_or("package has no manifest_path")?,
        );
        let crate_root = manifest
            .parent()
            .ok_or("package manifest has no parent directory")?;
        let modfile = source
            .strip_prefix(crate_root)
            .map_err(|_| "rlib source is outside the crate root")?
            .to_path_buf();

        Ok(Package {
            source,
            id: package["id"]
                .as_str()
                .ok_or("package has no id")?
                .to_owned(),
            crate_name: target["name"]
                .as_str()
                .ok_or("rlib target has no name")?
                .to_owned(),
            modfile,
        })
    }

    pub fn build(
        &self,
        kernel: &KernelBuild,
        args: &[String],
        package: &Package,
    ) -> Result<(PathBuf, Vec<PathBuf>)> {
        let message_format_json = args.iter().any(|s| s == "--message-format=json");
        let args = args
            .iter()
            .filter(|s| *s != "--message-format=json")
            .cloned()
            .collect::<Vec<_>>();
        let args = args.as_slice();
        let mut command = self.command("build", kernel, args, &package.modfile);
        command
            .arg(if message_format_json {
                "--message-format=json"
            } else {
                "--message-format=json-render-diagnostics"
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let printable = format!("{command:?}");
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or("failed to capture Cargo output")?;

        let mut main_rlib = None;
        let mut deps_rlibs = Vec::new();

        for line in BufReader::new(stdout).lines() {
            let line = line?;
            if message_format_json {
                println!("{line}");
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                if !message_format_json {
                    println!("{line}");
                }
                continue;
            };

            match message["reason"].as_str() {
                Some("compiler-message") => {
                    if let Some(rendered) = message["message"]["rendered"].as_str() {
                        eprint!("{rendered}");
                    }
                }
                Some("compiler-artifact") => {
                    let rlib_path = if let Some(filenames) = message["filenames"].as_array() {
                        filenames
                            .iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .find(|filename| filename.extension() == Some(OsStr::new("rlib")))
                    } else {
                        None
                    };

                    if let Some(rlib) = rlib_path {
                        let is_main_package = message["package_id"].as_str() == Some(&package.id)
                            && message["target"]["name"].as_str() == Some(&package.crate_name);

                        if is_main_package {
                            main_rlib = Some(rlib);
                        } else if message["target"]["kind"].as_array().is_some_and(|kinds| {
                            kinds
                                .iter()
                                .any(|kind| kind.as_str().is_some_and(|kind| kind == "lib"))
                        }) {
                            deps_rlibs.push(rlib);
                        }
                    }
                }
                _ => {}
            }
        }

        let status = child.wait()?;
        if !status.success() {
            return Err(format!("command failed with {status}: {printable}").into());
        }

        let rlib = main_rlib.ok_or_else(|| {
            format!(
                "Cargo did not report an rlib artifact for `{}`",
                package.crate_name
            )
        })?;

        Ok((rlib, deps_rlibs))
    }

    pub fn expand(&self, kernel: &KernelBuild, args: &[String], modfile: &PathBuf) -> Result<()> {
        process::run(&mut self.command("expand", kernel, args, modfile))
    }

    pub fn check(&self, kernel: &KernelBuild, args: &[String], modfile: &PathBuf) -> Result<()> {
        process::run(&mut self.command("build", kernel, args, modfile))
    }

    fn command(
        &self,
        subcommand: &str,
        kernel: &KernelBuild,
        args: &[String],
        modfile: &PathBuf,
    ) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .arg(subcommand)
            .arg("-Zjson-target-spec")
            .arg("--target")
            .arg(kernel.rust_target())
            .args(args)
            .env("OBJTREE", &kernel.dir)
            .env("RUST_MODFILE", modfile)
            .env("NOK_KBUILD_DIR", &kernel.dir)
            .env("RUSTC_BOOTSTRAP", "1")
            .env(
                "CARGO_ENCODED_RUSTFLAGS",
                kernel.rust_flags().join("\u{1f}"),
            );
        command
    }
}
