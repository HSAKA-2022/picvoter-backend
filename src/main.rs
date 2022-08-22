#![allow(unused)]
use anyhow::Result;
use console::style;
use log::{info, error, warn};
use rocket::tokio;
use std::{hash::Hasher, io::Write, path::Path, path::PathBuf, time::Duration};
use twox_hash::XxHash64;

#[rocket::get("/")]
fn index() -> &'static str {
  "Hello, world!"
}

#[rocket::main]
async fn main() -> Result<()> {
  env_logger::Builder::new()
    .target(env_logger::Target::Stderr)
    .filter_level(log::LevelFilter::Info)
    .parse_env("PICVOTER_LOG")
    .format(|buf, record| {
      let level = match record.level() {
        log::Level::Info => style("info: ").bold().blue(),
        log::Level::Error => style("error: ").bold().red(),
        log::Level::Warn => style("warn: ").bold().yellow(),
        log::Level::Debug => style("debug: ").bold().blue(),
        log::Level::Trace => style("trace: ").bold().cyan(),
        _ => unreachable!(),
      };

      writeln!(buf, "{} {}", level, record.args())
    })
    .init();

  let storage_path =
    std::env::var("PICVOTER_STORAGE_DIR").unwrap_or_else(|_| "./storage".to_string());
  let imports_path = PathBuf::from(&storage_path).join("imports");
  let pictures_path = PathBuf::from(&storage_path).join("pictures");

  tokio::fs::create_dir_all(&imports_path).await?;
  tokio::fs::create_dir_all(&pictures_path).await?;

  let r = rocket::build()
    .mount("/", rocket::routes![index])
    .ignite()
    .await?;

  tokio::spawn(async move { import_task(imports_path, pictures_path).await });

  let _ = r.launch().await?;
  Ok(())
}

async fn import_task(imports_path: PathBuf, pictures_path: PathBuf) {
  loop {
    info!("Checking for new imports...");
    match check_imports(&imports_path, &pictures_path).await {
      Ok(_) => info!("Imports check complete"),
      Err(err) => error!("Failed to check imports: {err:?}"),
    };

    tokio::time::sleep(Duration::from_secs(5)).await;
  }
}

async fn check_imports(imports_path: &Path, pictures_path: &Path) -> Result<()> {
  let mut files = tokio::fs::read_dir(&imports_path).await?;
  while let Some(file) = files.next_entry().await? {
    if !file.path().is_file() {
      continue;
    }

    let ext = match file.path().extension() {
        Some(ext) => ext,
        None => {
          warn!("No extension found for file {file:?}");
          continue;
        },
    };

    let data = tokio::fs::read(file.path()).await?;
    let mut hasher = XxHash64::with_seed(0);
    hasher.write(&data);

    let file_hash = hasher.finish();
    info!("Found new file: {file:?} with hash {file_hash}");
  }

  Ok(())
}
