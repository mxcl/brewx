use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use serde::Deserialize;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;

const DB_PATH: &str = "db.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct Db {
    schema: u32,
    #[allow(dead_code)]
    generated_at: String,
    entries: HashMap<String, Vec<DbEntry>>,
}

#[derive(Debug, Deserialize)]
struct DbEntry {
    formula: String,
    #[allow(dead_code)]
    popularity: Option<u64>,
}

fn main() {
    let mut args = env::args_os();
    let program = args
        .next()
        .unwrap_or_else(|| OsString::from("brewx"));

    let Some(tool_os) = args.next() else {
        print_usage(&program);
        process::exit(64);
    };

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

    let tool_args: Vec<OsString> = args.collect();
    let db_path = Path::new(DB_PATH);

    if !db_path.exists() {
        eprintln!(
            "brewx: expected {} in the current directory. Run ./build-db.py first.",
            DB_PATH
        );
        process::exit(2);
    }

    let db = match load_db(db_path) {
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

    let Some(entry) = select_entry(&db, &tool) else {
        eprintln!("brewx: no Homebrew formula found for '{tool}'");
        process::exit(1);
    };

    if let Some(path) = find_in_path(&tool) {
        exec_tool(&path, &tool_args);
    }

    if let Err(err) = run_brew_install(&entry.formula) {
        eprintln!("brewx: {err}");
        process::exit(1);
    }

    exec_tool(&tool, &tool_args);
}

fn load_db(path: &Path) -> Result<Db, String> {
    let data = fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    serde_json::from_slice(&data)
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))
}

fn select_entry<'a>(db: &'a Db, tool: &str) -> Option<&'a DbEntry> {
    db.entries.get(tool).and_then(|entries| entries.first())
}

fn is_help_flag(value: &OsString) -> bool {
    matches!(value.to_str(), Some("-h" | "--help"))
}

fn is_version_flag(value: &OsString) -> bool {
    matches!(value.to_str(), Some("-V" | "--version"))
}

fn print_usage(program: &OsString) {
    let program = program.to_string_lossy();
    println!("Usage: {program} <executable> [args...]");
    println!();
    println!("Runs a Homebrew executable, installing its formula if needed.");
    println!("Requires a db.json file in the current directory.");
}

fn find_in_path(tool: &str) -> Option<PathBuf> {
    if tool.contains('/') {
        let path = PathBuf::from(tool);
        if is_executable(&path) {
            return Some(path);
        }
        return None;
    }

    let path_var = env::var_os("PATH")?;
    for entry in env::split_paths(&path_var) {
        let candidate = entry.join(tool);
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
    let err = Command::new(tool).args(args).exec();
    eprintln!("brewx: failed to exec: {err}");
    process::exit(1);
}
