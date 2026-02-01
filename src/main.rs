use std::collections::HashMap;
use std::env;
use std::fs;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use serde::Deserialize;
use std::os::unix::fs::PermissionsExt;
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
    let program = args
        .next()
        .unwrap_or_else(|| OsString::from("brewx"));

    let Some(mut tool_os) = args.next() else {
        print_usage(&program);
        process::exit(64);
    };

    let mut shebang_mode = false;
    if is_shebang_flag(&tool_os) {
        shebang_mode = true;
        tool_os = match args.next() {
            Some(value) => value,
            None => {
                print_usage(&program);
                process::exit(64);
            }
        };
    }

    if is_help_flag(&tool_os) {
        print_usage(&program);
        return;
    }

    if is_version_flag(&tool_os) {
        println!("brewx 0.1.0");
        return;
    }

    let tool = match tool_os.to_str() {
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

    if tool.contains('/') {
        eprintln!("brewx: tool name must not contain path separators");
        process::exit(64);
    }

    let prefix = match brew_prefix() {
        Ok(prefix) => prefix,
        Err(err) => {
            eprintln!("brewx: {err}");
            process::exit(1);
        }
    };

    if let Some(path) = find_brew_executable(&prefix, formula, &tool) {
        exec_tool(&path, &tool_args);
    }

    if is_root() {
        eprintln!("brewx: refusing to run brew install as root");
        process::exit(1);
    }

    if let Err(err) = run_brew_install(formula) {
        eprintln!("brewx: {err}");
        process::exit(1);
    }

    if let Some(path) = find_brew_executable(&prefix, formula, &tool) {
        exec_tool(&path, &tool_args);
    }

    eprintln!(
        "brewx: '{tool}' not found under Homebrew prefix {}",
        prefix.display()
    );
    process::exit(1);
}

fn load_db() -> Result<Db, String> {
    serde_json::from_slice(EMBEDDED_DB)
        .map_err(|err| format!("failed to parse embedded db: {err}"))
}

fn select_entry<'a>(db: &'a Db, tool: &str) -> Option<&'a str> {
    db.entries.get(tool).map(|value| value.as_str())
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
    println!("Usage: {program} [-! | --shebang] <executable> [args...]");
    println!();
    println!("Runs a Homebrew executable, installing its formula if needed.");
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

    let output = Command::new("brew")
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

fn homebrew_bin_path_env() -> Option<OsString> {
    let homebrew_bin = PathBuf::from("/opt/homebrew/bin");
    let path_os = env::var_os("PATH");
    if let Some(ref current) = path_os {
        for entry in env::split_paths(current) {
            if entry == homebrew_bin {
                return None;
            }
        }
    }

    let mut paths = Vec::new();
    paths.push(homebrew_bin);
    if let Some(current) = path_os {
        paths.extend(env::split_paths(&current));
    }
    env::join_paths(paths).ok()
}

fn run_brew_install(formula: &str) -> Result<(), String> {
    eprintln!("brewx: installing {formula} via brew");
    let status = Command::new("brew")
        .arg("install")
        .arg(formula)
        .status()
        .map_err(|err| format!("failed to run brew: {err}"))?;

    if status.success() {
        return Ok(());
    }

    Err(match status.code() {
        Some(code) => format!("brew install {formula} failed with exit code {code}"),
        None => format!("brew install {formula} terminated by signal"),
    })
}

fn exec_tool<T: AsRef<OsStr>>(tool: T, args: &[OsString]) -> ! {
    let mut cmd = Command::new(tool);
    cmd.args(args);
    if let Some(path) = homebrew_bin_path_env() {
        cmd.env("PATH", path);
    }
    let err = cmd.exec();
    eprintln!("brewx: failed to exec: {err}");
    process::exit(1);
}
