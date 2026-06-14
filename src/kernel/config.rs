use std::{collections::HashMap, fs, path::Path};

use crate::Result;

pub struct KernelConfig {
    values: HashMap<String, String>,
}

impl KernelConfig {
    #[cfg(test)]
    pub fn from_values(values: HashMap<String, String>) -> Self {
        Self { values }
    }

    pub fn read(path: &Path) -> Result<Self> {
        let values = fs::read_to_string(path)?
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.trim_matches('"').to_owned()))
            .collect();
        Ok(Self { values })
    }

    pub fn enabled(&self, key: &str) -> bool {
        self.values.get(key).is_some_and(|value| value == "y")
    }

    pub fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn version_at_least(&self, key: &str, major: u32, minor: u32, patch: u32) -> bool {
        let expected = 100_000 * major + 100 * minor + patch;
        self.value(key)
            .and_then(|version| version.parse::<u32>().ok())
            .is_some_and(|version| version >= expected)
    }

    pub fn objtool_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let add = |args: &mut Vec<String>, config: &str, argument: &str| {
            if self.enabled(config) {
                args.push(argument.to_owned());
            }
        };

        add(
            &mut args,
            "CONFIG_HAVE_JUMP_LABEL_HACK",
            "--hacks=jump_label",
        );
        add(&mut args, "CONFIG_HAVE_NOINSTR_HACK", "--hacks=noinstr");
        add(
            &mut args,
            "CONFIG_MITIGATION_CALL_DEPTH_TRACKING",
            "--hacks=skylake",
        );
        add(&mut args, "CONFIG_X86_KERNEL_IBT", "--ibt");
        add(&mut args, "CONFIG_FINEIBT", "--cfi");
        add(&mut args, "CONFIG_FTRACE_MCOUNT_USE_OBJTOOL", "--mcount");
        if self.enabled("CONFIG_FTRACE_MCOUNT_USE_OBJTOOL") {
            add(&mut args, "CONFIG_HAVE_OBJTOOL_NOP_MCOUNT", "--mnop");
        }
        add(&mut args, "CONFIG_UNWINDER_ORC", "--orc");
        add(&mut args, "CONFIG_MITIGATION_RETPOLINE", "--retpoline");
        add(&mut args, "CONFIG_MITIGATION_RETHUNK", "--rethunk");
        add(&mut args, "CONFIG_MITIGATION_SLS", "--sls");
        add(&mut args, "CONFIG_STACK_VALIDATION", "--stackval");
        add(&mut args, "CONFIG_HAVE_STATIC_CALL_INLINE", "--static-call");
        add(&mut args, "CONFIG_HAVE_UACCESS_VALIDATION", "--uaccess");
        if self.enabled("CONFIG_GCOV_KERNEL") || self.enabled("CONFIG_KCOV") {
            args.push("--no-unreachable".into());
        }
        if self.enabled("CONFIG_PREFIX_SYMBOLS")
            && let Some(bytes) = self.values.get("CONFIG_FUNCTION_PADDING_BYTES")
        {
            args.push(format!("--prefix={bytes}"));
        }
        add(&mut args, "CONFIG_OBJTOOL_WERROR", "--werror");
        if self.enabled("CONFIG_LTO_CLANG")
            || self.enabled("CONFIG_X86_KERNEL_IBT")
            || self.enabled("CONFIG_KLP_BUILD")
        {
            args.push("--link".into());
        }
        args.push("--module".into());
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_objtool_arguments_from_kernel_config() {
        let config = KernelConfig {
            values: HashMap::from([
                ("CONFIG_OBJTOOL".into(), "y".into()),
                ("CONFIG_X86_KERNEL_IBT".into(), "y".into()),
                ("CONFIG_UNWINDER_ORC".into(), "y".into()),
                ("CONFIG_PREFIX_SYMBOLS".into(), "y".into()),
                ("CONFIG_FUNCTION_PADDING_BYTES".into(), "16".into()),
            ]),
        };

        assert_eq!(
            config.objtool_args(),
            ["--ibt", "--orc", "--prefix=16", "--link", "--module"]
        );
    }
}
