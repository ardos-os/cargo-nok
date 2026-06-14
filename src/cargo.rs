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
    ) -> Result<(PathBuf, Vec<PathBuf>)> { // 1. Alterada a assinatura de retorno
        dbg!(args);
        let message_format_json = args.iter().any(|s| s == "--message-format=json");
        let args = args.iter().filter(|s| *s != "--message-format=json").cloned().collect::<Vec<_>>();
        let args = args.as_slice();
        let mut command = self.command("build", kernel, args, module_name);
        command
            .arg(if message_format_json {"--message-format=json"} else {"--message-format=json-render-diagnostics"})
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let printable = format!("{command:?}");
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or("failed to capture Cargo output")?;
        
        let mut main_rlib = None;
        let mut deps_rlibs = Vec::new(); // 2. Vetor para guardar as dependências

        for line in BufReader::new(stdout).lines() {
            let line = line?;
            if message_format_json {
                println!("{line}");
                continue;
            }
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
                Some("compiler-artifact") => {
                    // Extrai o .rlib se existir neste artifact
                    let rlib_path = if let Some(filenames) = message["filenames"].get::<Vec<JsonValue>>() {
                        filenames
                            .iter()
                            .filter_map(|filename| filename.get::<String>())
                            .map(PathBuf::from)
                            .find(|filename| filename.extension() == Some(OsStr::new("rlib")))
                    } else {
                        None
                    };

                    // Se encontrámos um .rlib, vamos ver a quem pertence
                    if let Some(rlib) = rlib_path {
                        let is_main_package = message["package_id"].get::<String>() == Some(&package.id)
                            && message["target"]["name"].get::<String>() == Some(&package.crate_name);

                        if is_main_package {
                            main_rlib = Some(rlib); // É o teu módulo
                        } else {
                            // É uma dependência! Mas excluímos scripts de build ou binários acidentais
                            if message["target"]["kind"].get::<Vec<JsonValue>>().map_or(false, |kinds| {
                                kinds.iter().any(|k| k.get::<String>().is_some_and(|s| s == "lib"))
                            }) {
                                deps_rlibs.push(rlib);
                            }
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

        Ok((rlib, deps_rlibs)) // 3. Devolvemos ambos
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
