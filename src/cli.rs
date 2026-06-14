use std::path::PathBuf;

use crate::Result;

pub fn validate_cargo_args(args: &[String]) -> Result<()> {
    for reserved in ["--target"] {
        if option_present(args, reserved) {
            return Err(
                format!("`{reserved}` is managed by cargo-nok and cannot be overridden").into(),
            );
        }
    }
    Ok(())
}

pub fn option_present(args: &[String], option: &str) -> bool {
    args.iter()
        .any(|arg| arg == option || arg.starts_with(&format!("{option}=")))
}

pub fn path_option(args: &[String], option: &str) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == option)
        .map(|pair| PathBuf::from(&pair[1]))
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.strip_prefix(&format!("{option}=")))
                .map(PathBuf::from)
        })
}

pub fn string_option(args: &[String], long: &str, short: Option<&str>) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == long || short.is_some_and(|short| pair[0] == short))
        .map(|pair| pair[1].clone())
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.strip_prefix(&format!("{long}=")))
                .map(str::to_owned)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved_cargo_options() {
        assert!(validate_cargo_args(&["--target".into(), "custom.json".into()]).is_err());
        assert!(validate_cargo_args(&["--target=custom.json".into()]).is_err());
        assert!(validate_cargo_args(&["--release".into()]).is_ok());
    }

    #[test]
    fn reads_short_and_long_package_options() {
        assert_eq!(
            string_option(&["-p".into(), "driver".into()], "--package", Some("-p")),
            Some("driver".into())
        );
        assert_eq!(
            string_option(&["--package=driver".into()], "--package", Some("-p")),
            Some("driver".into())
        );
    }
}
