#![allow(unused)]
use anyhow::Result;
use console::style;
use image::imageops::FilterType;
use log::{error, info, warn};
use rocket::{
  serde::json::{json, Value},
  tokio,
};
use std::{hash::Hasher, io::Write, path::Path, path::PathBuf, time::Duration};
use twox_hash::XxHash64;

#[rocket::get("/")]
fn index() -> Value {
  json!({ "id": "some", "url": "some" })
}

#[rocket::post("/vote")]
fn vote() -> Value {
  json!({ "success": true })
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
  let raws_path = PathBuf::from(&storage_path).join("raws");
  let resized_path = PathBuf::from(&storage_path).join("resized");

  tokio::fs::create_dir_all(&imports_path).await?;
  tokio::fs::create_dir_all(&raws_path).await?;
  tokio::fs::create_dir_all(&resized_path).await?;

  let r = rocket::build()
    .mount("/", rocket::routes![index, vote])
    .ignite()
    .await?;

  tokio::spawn(async move { import_task(imports_path, raws_path, resized_path).await });

  let _ = r.launch().await?;
  Ok(())
}

async fn import_task(imports_path: PathBuf, raws_path: PathBuf, resized_path: PathBuf) {
  loop {
    info!("Checking for new imports...");
    match check_imports(&imports_path, &raws_path, &resized_path).await {
      Ok(_) => info!("Imports check complete"),
      Err(err) => error!("Failed to check imports: {err:?}"),
    };

    tokio::time::sleep(Duration::from_secs(5)).await;
  }
}

async fn check_imports(imports_path: &Path, raws_path: &Path, resized_path: &Path) -> Result<()> {
  let mut files = tokio::fs::read_dir(&imports_path).await?;
  while let Some(file) = files.next_entry().await? {
    let path = file.path();
    if !path.is_file() {
      continue;
    }

    let ext = match path.extension() {
      Some(ext) => ext,
      None => {
        warn!("No extension found for file {file:?}");
        continue;
      }
    };

    let ext = ext.to_string_lossy().to_string();
    let data = tokio::fs::read(file.path()).await?;
    let mut hasher = XxHash64::with_seed(0);
    hasher.write(&data);

    let file_hash = hasher.finish();
    info!("Found new file: {file:?} with hash {file_hash}");

    let new_file_name = format!("{file_hash}.{ext}");
    let new_path = raws_path.to_owned().join(&new_file_name);
    if new_path.exists() {
      info!("File {new_path:?} was already imported ({file:?}), removing");
      tokio::fs::remove_file(&path).await?;
      continue;
    }

    tokio::fs::rename(&path, &new_path).await?;

    let resized_path = resized_path.join(&new_file_name);
    let img = image::open(&new_path)?;
    let img = img.resize_to_fill(1080, 1080, FilterType::Lanczos3);
    img.save_with_format(&resized_path, image::ImageFormat::Jpeg);
  }

  Ok(())
}
