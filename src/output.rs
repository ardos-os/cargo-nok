use colored::Colorize;

pub fn error(message: impl std::fmt::Display) {
    eprintln!("{} {message}", "error:".red().bold());
}

pub fn status(label: &str, message: impl std::fmt::Display) {
    eprintln!("{} {message}", format!("{label:>12}").green().bold());
}

pub fn warning(message: impl std::fmt::Display) {
    eprintln!("{} {message}", "warning:".yellow().bold());
}
