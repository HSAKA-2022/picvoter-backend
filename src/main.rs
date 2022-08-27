#![allow(unused)]
use console::style;
use image::imageops::FilterType;
use log::{error, info, warn};
use rocket::{
  http::Status,
  response::{self, Responder},
  serde::json::{json, Value},
  Request, State, fs::FileServer,
};
use serde::Serialize;
use sqlx::SqlitePool;
use std::{env, hash::Hasher, io::Write, path::Path, path::PathBuf, time::Duration};
use tokio::{self, fs};
use twox_hash::XxHash64;

#[derive(Debug)]
pub struct Error(pub anyhow::Error);

impl<E> From<E> for crate::Error
where
  E: Into<anyhow::Error>,
{
  fn from(error: E) -> Self {
    Error(error.into())
  }
}

impl<'r> Responder<'r, 'static> for Error {
  fn respond_to(self, request: &Request<'_>) -> response::Result<'static> {
    response::Debug(self.0).respond_to(request)
  }
}

pub type Result<T = ()> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize)]
struct ImageEntry {
  id: String,
  hash: String,
}

#[rocket::get("/?<count>")]
async fn index(db: &State<SqlitePool>, count: Option<u8>) -> Result<(Status, Value)> {
  let count = count.unwrap_or(1);
  if count > 10 {
    return Ok((
      Status::BadRequest,
      json!({ "error": "count must be less than 10" }),
    ));
  }

  let mut db = db.acquire().await?;
  let result = sqlx::query_as!(
    ImageEntry,
    r#"
SELECT id, hash
FROM images
LIMIT ?1
    "#,
    count,
  )
  .fetch_all(&mut db)
  .await?;

  Ok((Status::Ok, json!(result)))
}

#[rocket::post("/vote")]
fn vote() -> Value {
  json!({ "success": true })
}

#[derive(Debug, Clone)]
pub struct Config {
  raws_path: PathBuf,
  imports_path: PathBuf,
  resized_path: PathBuf,
}

#[tokio::main]
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

  let db_path =
    env::var("VOTER_DB_PATH").unwrap_or_else(|_| "sqlite:./storage/db.sqlite?mode=rwc".to_string());
  let pool = sqlx::SqlitePool::connect(&db_path).await?;
  sqlx::migrate!().run(&pool).await?;

  let imports_path =
    env::var("VOTER_IMPORTS_DIR").unwrap_or_else(|_| "./storage/imports".to_string());
  let raws_path = env::var("VOTER_RAWS_DIR").unwrap_or_else(|_| "./storage/raws".to_string());
  let resized_path =
    env::var("VOTER_RESIZED_DIR").unwrap_or_else(|_| "./storage/resized".to_string());

  let config = Config {
    raws_path: raws_path.into(),
    imports_path: imports_path.into(),
    resized_path: resized_path.clone().into(),
  };

  fs::create_dir_all(&config.imports_path).await?;
  fs::create_dir_all(&config.raws_path).await?;
  fs::create_dir_all(&config.resized_path).await?;

  let r = rocket::build()
    .manage(config.clone())
    .manage(pool.clone())
    .mount("/", rocket::routes![index, vote])
    .mount("/files", FileServer::from(&resized_path))
    .ignite()
    .await?;

  tokio::spawn(async move {
    loop {
      info!("Checking for new imports...");
      match check_imports(&config, &pool).await {
        Ok(_) => info!("Imports check complete"),
        Err(err) => error!("Failed to check imports: {err:?}"),
      };

      tokio::time::sleep(Duration::from_secs(5)).await;
    }
  });

  let _ = r.launch().await?;
  Ok(())
}

async fn save_image(
  config: &Config,
  db: &SqlitePool,
  original_image_path: &Path,
  raw_image_path: &Path,
  hash: u64,
) -> Result<()> {
  fs::copy(&original_image_path, &raw_image_path).await?;

  let original_filename = original_image_path
    .file_name()
    .unwrap_or_default()
    .to_string_lossy()
    .to_string();

  let resized_filename = format!("{hash}.jpg");
  let resized_path = config.resized_path.join(&resized_filename);
  let img = image::open(&raw_image_path)?;
  let resized_img = img.resize_to_fill(1080, 1080, FilterType::Lanczos3);
  resized_img.save(&resized_path)?;

  let image_id = ulid::Ulid::new().to_string();
  let hash_str = hash.to_string();

  let mut conn = db.acquire().await?;
  sqlx::query!(
    r#"
INSERT INTO images ( id, filename, hash )
VALUES ( ?1, ?2, ?3 )
    "#,
    image_id,
    original_filename,
    hash_str,
  )
  .execute(&mut conn).await?;

  Ok(())
}

async fn check_imports(config: &Config, db: &SqlitePool) -> Result<()> {
  let mut files = fs::read_dir(&config.imports_path).await?;

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
    let data = fs::read(file.path()).await?;
    let mut hasher = XxHash64::with_seed(0);
    hasher.write(&data);

    let file_hash = hasher.finish();
    info!("Found new file: {file:?} with hash {file_hash}");

    let new_file_name = format!("{file_hash}.{ext}");
    let raw_image_path = config.raws_path.to_owned().join(&new_file_name);
    if raw_image_path.exists() {
      info!("File {raw_image_path:?} was already imported ({file:?}), skipping");
      continue;
    }

    save_image(config, db, &path, &raw_image_path, file_hash).await?;
  }

  Ok(())
}
