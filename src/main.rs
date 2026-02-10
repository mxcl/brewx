use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use serde::Deserialize;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::CommandExt;

const SCHEMA_VERSION: u32 = 2;
const EMBEDDED_DB: &[u8] = include_bytes!("../db.json");

#[derive(Debug, Deserialize)]
struct Db {
    schema: u32,
    #[allow(dead_code)]
    generated_at: String,
    entries: HashMap<String, String>,
}

fn main() {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_else(|| OsString::from("brewx"));

    let Some(mut first_arg) = args.next() else {
        print_usage(&program);
        process::exit(64);
    };

    let mut shebang_mode = false;
    if is_shebang_flag(&first_arg) {
        shebang_mode = true;
        first_arg = match args.next() {
            Some(value) => value,
            None => {
                print_usage(&program);
                process::exit(64);
            }
        };
    }

    if is_help_flag(&first_arg) {
        print_usage(&program);
        return;
    }

    if is_version_flag(&first_arg) {
        println!("brewx {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let mut formulas = Vec::new();
    while let Some(formula) = match parse_formula_spec(&first_arg) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("brewx: {err}");
            process::exit(64);
        }
    } {
        push_unique_string(&mut formulas, formula);
        first_arg = match args.next() {
            Some(value) => value,
            None => {
                print_usage(&program);
                process::exit(64);
            }
        };
    }
    let has_formula_overrides = !formulas.is_empty();

    let tool = match first_arg.to_str() {
        Some(value) => value.to_string(),
        None => {
            eprintln!("brewx: tool name must be valid UTF-8");
            process::exit(64);
        }
    };

    if shebang_mode {
        let _ = args.next();
    }
    let tool_args: Vec<OsString> = args.collect();

    if tool.contains('/') {
        eprintln!("brewx: tool name must not contain path separators");
        process::exit(64);
    }

    if formulas.is_empty() {
        let db = match load_db() {
            Ok(db) => db,
            Err(err) => {
                eprintln!("brewx: {err}");
                process::exit(1);
            }
        };

        if db.schema != SCHEMA_VERSION {
            eprintln!(
                "brewx: unsupported db schema {} (expected {})",
                db.schema, SCHEMA_VERSION
            );
            process::exit(1);
        }

        let Some(formula) = select_entry(&db, &tool) else {
            eprintln!("brewx: no Homebrew formula found for '{tool}'");
            process::exit(1);
        };

        formulas.push(formula.to_string());
    }

    let prefix = match brew_prefix() {
        Ok(prefix) => prefix,
        Err(err) => {
            eprintln!("brewx: {err}");
            process::exit(1);
        }
    };

    if let Some(path) = find_brew_executable_for_formulas(&prefix, &formulas, &tool) {
        exec_tool(&path, &tool_args, &prefix, &formulas);
    }

    if is_root() {
        eprintln!("brewx: refusing to run brew install as root");
        process::exit(1);
    }

    if let Err(err) = install_formulas(&prefix, &formulas, !has_formula_overrides) {
        eprintln!("brewx: {err}");
        process::exit(1);
    }

    if let Some(path) = find_brew_executable_for_formulas(&prefix, &formulas, &tool) {
        exec_tool(&path, &tool_args, &prefix, &formulas);
    }

    eprintln!(
        "brewx: '{tool}' not found in formulae [{}]",
        formulas.join(", ")
    );
    process::exit(1);
}

fn load_db() -> Result<Db, String> {
    serde_json::from_slice(EMBEDDED_DB).map_err(|err| format!("failed to parse embedded db: {err}"))
}

fn select_entry<'a>(db: &'a Db, tool: &str) -> Option<&'a str> {
    db.entries.get(tool).map(|value| value.as_str())
}

fn parse_formula_spec(value: &OsString) -> Result<Option<String>, String> {
    let Some(value) = value.to_str() else {
        return Ok(None);
    };

    let Some(formula) = value.strip_prefix('+') else {
        return Ok(None);
    };

    if formula.is_empty() {
        return Err("package spec '+' is missing a formula name".to_string());
    }

    if formula.contains('/') {
        return Err(format!(
            "package spec '+{formula}' must not contain path separators"
        ));
    }

    Ok(Some(formula.to_string()))
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|entry| entry == &value) {
        values.push(value);
    }
}

fn is_help_flag(value: &OsString) -> bool {
    matches!(value.to_str(), Some("-h" | "--help"))
}

fn is_version_flag(value: &OsString) -> bool {
    matches!(value.to_str(), Some("-V" | "--version"))
}

fn is_shebang_flag(value: &OsString) -> bool {
    matches!(value.to_str(), Some("-!" | "--shebang"))
}

fn print_usage(program: &OsString) {
    let program = program.to_string_lossy();
    println!("Usage: {program} [-! | --shebang] [+formula ...] <executable> [args...]");
    println!();
    println!("Runs a Homebrew executable, installing its formula if needed.");
    println!("Use +formula to select Homebrew formulae, pkgx-style.");
    println!("Use -! or --shebang to drop the first argument (script path).");
    println!("Uses an embedded Homebrew executable map.");
    println!("Only executes binaries under the Homebrew prefix.");
}

fn brew_prefix() -> Result<PathBuf, String> {
    if let Some(prefix) = env::var_os("HOMEBREW_PREFIX") {
        if !prefix.is_empty() {
            return Ok(PathBuf::from(prefix));
        }
    }

    let output = brew_command()
        .arg("--prefix")
        .output()
        .map_err(|err| format!("failed to run brew --prefix: {err}"))?;

    if !output.status.success() {
        return Err("brew --prefix failed".to_string());
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|err| format!("brew --prefix returned non-utf8: {err}"))?;
    let prefix = stdout.trim();
    if prefix.is_empty() {
        return Err("brew --prefix returned an empty prefix".to_string());
    }
    Ok(PathBuf::from(prefix))
}

fn find_brew_executable(prefix: &Path, formula: &str, tool: &str) -> Option<PathBuf> {
    let candidates = [
        prefix.join("bin").join(tool),
        prefix.join("sbin").join(tool),
        prefix.join("opt").join(formula).join("bin").join(tool),
        prefix.join("opt").join(formula).join("sbin").join(tool),
    ];

    for candidate in candidates {
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    find_cellar_executable(prefix, formula, tool)
}

fn find_brew_executable_for_formulas(
    prefix: &Path,
    formulas: &[String],
    tool: &str,
) -> Option<PathBuf> {
    for formula in formulas {
        if let Some(path) = find_brew_executable(prefix, formula, tool) {
            return Some(path);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    if !metadata.is_file() {
        return false;
    }
    metadata.permissions().mode() & 0o111 != 0
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn add_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|entry| entry == &path) {
        paths.push(path);
    }
}

fn add_opt_paths(paths: &mut Vec<PathBuf>, prefix: &Path, formula: &str) {
    let opt = prefix.join("opt").join(formula);
    add_unique_path(paths, opt.join("bin"));
    add_unique_path(paths, opt.join("sbin"));
}

fn latest_cellar_keg(prefix: &Path, formula: &str) -> Option<PathBuf> {
    let cellar = prefix.join("Cellar").join(formula);
    let entries = fs::read_dir(&cellar).ok()?;
    let mut versions = Vec::new();
    for entry in entries {
        let entry = entry.ok()?;
        if entry.file_type().ok()?.is_dir() {
            versions.push(entry.path());
        }
    }
    if versions.is_empty() {
        return None;
    }
    versions.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
    });
    versions.pop()
}

fn add_cellar_paths(paths: &mut Vec<PathBuf>, prefix: &Path, formula: &str) {
    if let Some(keg) = latest_cellar_keg(prefix, formula) {
        add_unique_path(paths, keg.join("bin"));
        add_unique_path(paths, keg.join("sbin"));
    }
}

fn find_cellar_executable(prefix: &Path, formula: &str, tool: &str) -> Option<PathBuf> {
    let keg = latest_cellar_keg(prefix, formula)?;
    let candidates = [keg.join("bin").join(tool), keg.join("sbin").join(tool)];
    for candidate in candidates {
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn brew_dependencies(formula: &str) -> Result<Vec<String>, String> {
    let output = brew_command()
        .arg("deps")
        .arg("--topological")
        .arg("--formula")
        .arg(formula)
        .output()
        .map_err(|err| format!("failed to run brew deps --topological {formula}: {err}"))?;

    if !output.status.success() {
        return Err(match output.status.code() {
            Some(code) => {
                format!("brew deps --topological {formula} failed with exit code {code}")
            }
            None => format!("brew deps --topological {formula} terminated by signal"),
        });
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|err| format!("brew deps --topological {formula} returned non-utf8: {err}"))?;
    let deps = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect();
    Ok(deps)
}

fn build_exec_path(prefix: &Path, formulas: &[String]) -> OsString {
    let mut paths = Vec::new();
    add_unique_path(&mut paths, PathBuf::from("/opt/homebrew/bin"));
    for formula in formulas {
        add_opt_paths(&mut paths, prefix, formula);
        add_cellar_paths(&mut paths, prefix, formula);
    }

    for formula in formulas {
        match brew_dependencies(formula) {
            Ok(deps) => {
                for dep in deps {
                    add_opt_paths(&mut paths, prefix, &dep);
                    add_cellar_paths(&mut paths, prefix, &dep);
                }
            }
            Err(err) => eprintln!("brewx: {err}"),
        }
    }

    if let Some(current) = env::var_os("PATH") {
        for entry in env::split_paths(&current) {
            add_unique_path(&mut paths, entry);
        }
    }

    env::join_paths(paths).unwrap_or_else(|_| env::var_os("PATH").unwrap_or_else(OsString::new))
}

fn install_formulas(
    prefix: &Path,
    formulas: &[String],
    reinstall_first: bool,
) -> Result<(), String> {
    for (index, formula) in formulas.iter().enumerate() {
        if !reinstall_first || index != 0 {
            if latest_cellar_keg(prefix, formula).is_some() {
                continue;
            }
        }
        run_brew_install(prefix, formula)?;
    }
    Ok(())
}

fn run_brew_install(prefix: &Path, formula: &str) -> Result<(), String> {
    eprintln!("brewx: installing {formula} via brew");
    let deps = brew_dependencies(formula)?;
    if !deps.is_empty() {
        eprintln!(
            "brewx: installing {} dependencies via brew --skip-link --as-dependency",
            deps.len()
        );
        run_brew_install_deps(prefix, &deps)?;
    }

    run_brew_install_skip_link(prefix, formula, false, false)
}

fn run_brew_install_deps(prefix: &Path, deps: &[String]) -> Result<(), String> {
    let mut cmd = brew_command();
    cmd.arg("install").arg("--skip-link").arg("--as-dependency");
    for dep in deps {
        cmd.arg(dep);
    }

    let status = cmd
        .status()
        .map_err(|err| format!("failed to run brew: {err}"))?;

    if !status.success() {
        return Err(match status.code() {
            Some(code) => {
                format!("brew install --skip-link --as-dependency failed with exit code {code}")
            }
            None => "brew install --skip-link --as-dependency terminated by signal".to_string(),
        });
    }

    for dep in deps {
        ensure_opt_link(prefix, dep)?;
    }
    Ok(())
}

fn run_brew_install_skip_link(
    prefix: &Path,
    formula: &str,
    ignore_deps: bool,
    as_dependency: bool,
) -> Result<(), String> {
    let mut cmd = brew_command();
    cmd.arg("install").arg("--skip-link");
    if ignore_deps {
        cmd.arg("--ignore-dependencies");
    }
    if as_dependency {
        cmd.arg("--as-dependency");
    }
    cmd.arg(formula);

    let status = cmd
        .status()
        .map_err(|err| format!("failed to run brew: {err}"))?;

    if status.success() {
        ensure_opt_link(prefix, formula)?;
        return Ok(());
    }

    let mut description = String::from("brew install --skip-link");
    if ignore_deps {
        description.push_str(" --ignore-dependencies");
    }
    if as_dependency {
        description.push_str(" --as-dependency");
    }
    description.push(' ');
    description.push_str(formula);

    Err(match status.code() {
        Some(code) => format!("{description} failed with exit code {code}"),
        None => format!("{description} terminated by signal"),
    })
}

fn ensure_opt_link(prefix: &Path, formula: &str) -> Result<(), String> {
    let keg = latest_cellar_keg(prefix, formula)
        .ok_or_else(|| format!("no Cellar keg found for {formula}"))?;
    let opt_dir = prefix.join("opt");
    let link = opt_dir.join(formula);

    if link.exists() {
        let metadata = fs::symlink_metadata(&link)
            .map_err(|err| format!("failed to stat {}: {err}", link.display()))?;
        if metadata.file_type().is_symlink() {
            let current = fs::read_link(&link)
                .map_err(|err| format!("failed to read link {}: {err}", link.display()))?;
            if current == keg {
                return Ok(());
            }
            fs::remove_file(&link)
                .map_err(|err| format!("failed to remove {}: {err}", link.display()))?;
        } else {
            return Err(format!(
                "refusing to replace non-symlink opt path {}",
                link.display()
            ));
        }
    }

    fs::create_dir_all(&opt_dir)
        .map_err(|err| format!("failed to create {}: {err}", opt_dir.display()))?;
    symlink(&keg, &link).map_err(|err| {
        format!(
            "failed to link {} -> {}: {err}",
            link.display(),
            keg.display()
        )
    })
}

fn brew_command() -> Command {
    let mut cmd = Command::new("brew");
    cmd.env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1");
    cmd
}

fn exec_tool<T: AsRef<OsStr>>(tool: T, args: &[OsString], prefix: &Path, formulas: &[String]) -> ! {
    let mut cmd = Command::new(tool);
    cmd.args(args);
    cmd.env("PATH", build_exec_path(prefix, formulas));
    let err = cmd.exec();
    eprintln!("brewx: failed to exec: {err}");
    process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_formula_spec_handles_pkgx_style_prefix() {
        let value = OsString::from("+ffmpeg_full");
        let parsed = parse_formula_spec(&value).unwrap();
        assert_eq!(parsed, Some("ffmpeg_full".to_string()));
    }

    #[test]
    fn parse_formula_spec_ignores_non_prefixed_tokens() {
        let value = OsString::from("ffmpeg");
        let parsed = parse_formula_spec(&value).unwrap();
        assert_eq!(parsed, None);
    }

    #[test]
    fn parse_formula_spec_rejects_empty_formula_name() {
        let value = OsString::from("+");
        let parsed = parse_formula_spec(&value);
        assert_eq!(
            parsed,
            Err("package spec '+' is missing a formula name".to_string())
        );
    }

    #[test]
    fn parse_formula_spec_rejects_path_separator() {
        let value = OsString::from("+foo/bar");
        let parsed = parse_formula_spec(&value);
        assert_eq!(
            parsed,
            Err("package spec '+foo/bar' must not contain path separators".to_string())
        );
    }
}
