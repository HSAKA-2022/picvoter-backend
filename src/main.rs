use console::style;
use image::imageops::FilterType;
use log::{error, info, warn};
use rand::Rng;
use rocket::{
  fs::FileServer,
  http::Status,
  response::{self, Responder},
  serde::json::{json, Json, Value},
  Request, State,
};
use rocket_cors::{AllowedHeaders, AllowedOrigins};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{env, hash::Hasher, io::Write, path::Path, path::PathBuf, str::FromStr, time::Duration};
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

#[rocket::get("/")]
async fn index(db: &State<SqlitePool>) -> Result<(Status, Value)> {
  let next = get_next_pic(db).await?;
  if next.is_none() {
    return Ok((
      Status::InternalServerError,
      json!({ "error": "no picture found" }),
    ));
  }

  let next = next.unwrap();
  Ok((Status::Ok, json!(next)))
}

fn biased_random(max: i32) -> i32 {
  if max == 0 {
    return 0;
  }

  let mut rng = rand::thread_rng();

  let n = 4f32;
  let unif: f32 = rng.gen();

  let one_over2_n = 1.0 / f32::powf(2.0, n);
  let one_over_xplus1_n = 1.0 / f32::powf(unif + 1.0, n);

  let random = (one_over_xplus1_n - one_over2_n) / (1.0 - one_over2_n);
  (random * max as f32) as i32
}

async fn get_next_pic(db: &SqlitePool) -> Result<Option<ImageEntry>> {
  let mut db = db.acquire().await?;
  let count = sqlx::query!("SELECT COUNT(*) AS count FROM images WHERE sorting >= -3")
    .fetch_one(&mut db)
    .await?;

  let skip = biased_random(count.count);
  let result = sqlx::query!(
    r#"
SELECT id, hash
FROM images
WHERE sorting >= -3
ORDER BY confidence ASC
LIMIT 1
OFFSET ?1
    "#,
    skip
  )
  .fetch_optional(&mut db)
  .await?;

  if result.is_none() {
    return Ok(None);
  }

  let result = result.unwrap();

  Ok(Some(ImageEntry {
    id: result.id.unwrap(),
    hash: result.hash.unwrap(),
  }))
}

#[derive(Debug, Clone, Deserialize)]
struct VoteRequest {
  id: String,
  value: i8,
}

#[rocket::post("/vote", data = "<req>")]
async fn vote(db: &State<SqlitePool>, req: Json<VoteRequest>) -> Result<(Status, Value)> {
  let mut db = db.acquire().await?;
  let record = sqlx::query!(
    r#"
SELECT
  upvotes,
  downvotes,
  sorting,
  confidence
FROM images
WHERE id = ?1"#,
    req.id
  )
  .fetch_optional(&mut db)
  .await?;

  if record.is_none() {
    return Ok((Status::NotFound, json!({ "error": "not found" })));
  }

  let record = record.unwrap();
  let (upvotes, downvotes) = if req.value == 1 {
    (
      record.upvotes.unwrap_or_default() + 1,
      record.downvotes.unwrap_or_default(),
    )
  } else if req.value == -1 {
    (
      record.upvotes.unwrap_or_default(),
      record.downvotes.unwrap_or_default() + 1,
    )
  } else {
    return Ok((
      Status::BadRequest,
      json!({ "error": "value must be 1 or -1" }),
    ));
  };

  let new_sorting = calc_sort_value(upvotes, downvotes);
  let new_confidence = ((0.5 - new_sorting) * ((upvotes + downvotes) as f32 / 10.0)).abs();

  let _ = sqlx::query!(
    r#"
UPDATE images
SET
  upvotes = ?1,
  downvotes = ?2,
  sorting = ?3,
  confidence = ?4
WHERE
  id = ?5"#,
    upvotes,
    downvotes,
    new_sorting,
    new_confidence,
    req.id
  )
  .execute(&mut db)
  .await?;

  Ok((Status::Ok, json!({ "success": true })))
}

fn calc_sort_value(ups: i64, downs: i64) -> f32 {
  if ups == 0 {
    if downs == 0 {
      return 0.5;
    }

    return (downs * -1) as f32;
  }

  let n = (ups + downs) as f32;
  let z = 1.64485; //1.0 = 85%, 1.6 = 95%
  let phat = ups as f32 / n;

  (phat + z * z / (2.0 * n) - z * f32::sqrt((phat * (1.0 - phat) + z * z / (4.0 * n)) / n))
    / (1.0 + z * z / n)
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
    .parse_env("VOTER_LOG")
    .format(|buf, record| {
      let level = match record.level() {
        log::Level::Info => style("info: ").bold().blue(),
        log::Level::Error => style("error: ").bold().red(),
        log::Level::Warn => style("warn: ").bold().yellow(),
        log::Level::Debug => style("debug: ").bold().blue(),
        log::Level::Trace => style("trace: ").bold().cyan(),
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

  let cors = rocket_cors::CorsOptions {
    allowed_origins: AllowedOrigins::all(),
    allowed_methods: ["Get", "Post", "Head"]
      .iter()
      .map(|s| FromStr::from_str(s).unwrap())
      .collect(),
    allowed_headers: AllowedHeaders::all(),
    allow_credentials: true,
    ..Default::default()
  }
  .to_cors()?;

  let r = rocket::build()
    .manage(config.clone())
    .manage(pool.clone())
    .mount("/", rocket::routes![index, vote, resize_all_images])
    .mount("/files", FileServer::from(&resized_path))
    .attach(cors)
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

#[rocket::post("/resize_all")]
async fn resize_all_images(db: &State<SqlitePool>) -> Result<(Status, Value)> {
  let mut conn = db.acquire().await?;

  info!("Resizing all Images!");
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
  let records = sqlx::query!(r#"SELECT id, filename, hash FROM images"#,)
    .fetch_optional(&mut conn)
    .await?;

  for record in records {
    let raw_image_path = config.raws_path.join(record.hash.to_string() + ".png");
    info!("Resizing image: {}", &raw_image_path.clone().display());
    resize_img(
      &config,
      &raw_image_path,
      &record.hash.parse::<u64>().unwrap(),
    )
    .await?;
  }
  info!("Resizing Done");
  Ok((Status::Ok, json!({ "success": true })))
}

async fn save_image(
  config: &Config,
  db: &SqlitePool,
  original_image_path: &Path,
  raw_image_path: &Path,
  hash: u64,
) -> Result<()> {
  fs::copy(&original_image_path, &raw_image_path).await?;
  resize_img(&config, &raw_image_path, &hash).await?;

  let image_id = ulid::Ulid::new().to_string();
  let hash_str = hash.to_string();

  let original_filename = original_image_path
    .file_name()
    .unwrap_or_default()
    .to_string_lossy()
    .to_string();

  let mut conn = db.acquire().await?;
  sqlx::query!(
    r#"INSERT INTO images ( id, filename, hash ) VALUES ( ?1, ?2, ?3 )"#,
    image_id,
    original_filename,
    hash_str,
  )
  .execute(&mut conn)
  .await?;

  Ok(())
}

async fn resize_img(config: &Config, raw_image_path: &Path, hash: &u64) -> Result<()> {
  let resized_filename = format!("{hash}.jpg");
  let resized_path = config.resized_path.join(&resized_filename);
  let img = image::open(&raw_image_path)?;

  let resized_img = img.resize(1080, 1080, FilterType::Lanczos3);
  resized_img.save(&resized_path)?;

  Ok(())
}

async fn check_imports(config: &Config, db: &SqlitePool) -> Result<()> {
  let mut walk_dirs = vec![config.imports_path.clone()];
  while !walk_dirs.is_empty() {
    let dir = walk_dirs.pop().unwrap();
    let mut files = fs::read_dir(&dir).await?;

    while let Some(file) = files.next_entry().await? {
      let path = file.path();
      if path.is_dir() {
        walk_dirs.push(path.to_owned());
        continue;
      }

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

      let ext = ext.to_string_lossy().to_string().to_lowercase();
      match ext.as_str() {
        "jpg" | "jpeg" => {},
        other => {
          warn!("skipping extension {other}");
          continue;
        },
      };

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
  }

  Ok(())
}
