use layrs_cli::args::{CliCommand, parse_args, usage};
use layrs_store_local::{DEFAULT_STORE_DIR, LocalStore, LocalStoreConfig};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            eprintln!("{}", usage());
            std::process::exit(2);
        }
    };

    if let Err(error) = run(command) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(command: CliCommand) -> Result<(), String> {
    match command {
        CliCommand::Help => {
            println!("{}", usage());
            Ok(())
        }
        CliCommand::Init { store } => {
            let store = LocalStore::init(LocalStoreConfig::new(resolve_store_root(store)?))
                .map_err(|error| format!("failed to initialize store: {error}"))?;
            println!("initialized store at {}", store.layout().root.display());
            println!("metadata: {}", store.layout().metadata_dir.display());
            println!("objects: {}", store.layout().cas_dir.display());
            Ok(())
        }
        CliCommand::WorkspaceCreate { store, name } => {
            let store = open_or_init(store)?;
            let path = write_named_record(&store, "workspaces", &name, &[])?;
            println!("created workspace `{name}` at {}", path.display());
            Ok(())
        }
        CliCommand::SpaceCreate {
            store,
            workspace,
            name,
        } => {
            let store = open_or_init(store)?;
            let mut fields = Vec::new();
            if let Some(workspace) = workspace.as_deref() {
                fields.push(("workspace", workspace));
            }
            let path = write_named_record(&store, "spaces", &name, &fields)?;
            println!("created space `{name}` at {}", path.display());
            Ok(())
        }
        CliCommand::LayerCreate { store, space, name } => {
            let store = open_or_init(store)?;
            let mut fields = Vec::new();
            if let Some(space) = space.as_deref() {
                fields.push(("space", space));
            }
            let path = write_named_record(&store, "layers", &name, &fields)?;
            println!("created layer `{name}` at {}", path.display());
            Ok(())
        }
        CliCommand::Status { store } => {
            let root = resolve_store_root(store)?;
            if !root.exists() {
                println!("store not initialized at {}", root.display());
                return Ok(());
            }

            let store = LocalStore::open(LocalStoreConfig::new(root))
                .map_err(|error| format!("failed to open store: {error}"))?;
            println!("store: {}", store.layout().root.display());
            println!("sqlite skeleton: {}", store.layout().sqlite_path.display());
            println!("cas: {}", store.layout().cas_dir.display());
            Ok(())
        }
        CliCommand::StoreScrub { store } => {
            let store = LocalStore::open(LocalStoreConfig::new(resolve_store_root(store)?))
                .map_err(|error| format!("failed to open store: {error}"))?;
            let report = store
                .scrub()
                .map_err(|error| format!("failed to scrub store: {error}"))?;
            println!("checked_objects={}", report.checked_objects);
            println!("missing_objects={}", report.missing_objects);
            println!("unverified_objects={}", report.unverified_objects);
            println!("errors={}", report.errors.len());
            Ok(())
        }
    }
}

fn open_or_init(store: Option<PathBuf>) -> Result<LocalStore, String> {
    LocalStore::init_or_open(LocalStoreConfig::new(resolve_store_root(store)?))
        .map_err(|error| format!("failed to open store: {error}"))
}

fn resolve_store_root(store: Option<PathBuf>) -> Result<PathBuf, String> {
    match store {
        Some(store) => Ok(store),
        None => std::env::current_dir()
            .map(|cwd| cwd.join(DEFAULT_STORE_DIR))
            .map_err(|error| format!("failed to resolve current directory: {error}")),
    }
}

fn write_named_record(
    store: &LocalStore,
    collection: &str,
    name: &str,
    fields: &[(&str, &str)],
) -> Result<PathBuf, String> {
    let dir = store.layout().metadata_dir.join(collection);
    fs::create_dir_all(&dir)
        .map_err(|error| format!("failed to create {}: {error}", dir.display()))?;

    let path = unique_record_path(&dir, name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;

    writeln!(file, "kind={collection}").map_err(write_error)?;
    writeln!(file, "name={}", record_value(name)).map_err(write_error)?;
    writeln!(file, "created_at_unix_ms={}", now_unix_ms()).map_err(write_error)?;
    for (key, value) in fields {
        writeln!(file, "{key}={}", record_value(value)).map_err(write_error)?;
    }
    file.sync_all()
        .map_err(|error| format!("failed to sync {}: {error}", path.display()))?;

    Ok(path)
}

fn unique_record_path(dir: &Path, name: &str) -> PathBuf {
    let slug = slugify(name);
    let mut path = dir.join(format!("{slug}.txt"));
    let mut index = 2_u32;

    while path.exists() {
        path = dir.join(format!("{slug}-{index}.txt"));
        index += 1;
    }

    path
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "unnamed".to_string()
    } else {
        slug
    }
}

fn record_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn write_error(error: io::Error) -> String {
    format!("failed to write metadata record: {error}")
}
