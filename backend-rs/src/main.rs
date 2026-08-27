use actix_web::{web, App, HttpServer, HttpResponse, middleware};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

mod models;
mod handlers;
mod db;
mod regions;

use handlers::*;
use models::*;
pub struct AppState {
    pool: SqlitePool,
    summary_cache: Arc<std::sync::Mutex<Option<CachedResponse<SummaryResponse>>>>,
    network_total_cache: Arc<std::sync::Mutex<HashMap<String, CachedResponse<Vec<NetworkTotal>>>>>,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        AppState {
            pool: self.pool.clone(),
            summary_cache: Arc::clone(&self.summary_cache),
            network_total_cache: Arc::clone(&self.network_total_cache),
        }
    }
}

#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let database_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            format!(
                "sqlite://{}",
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string()))
                    .join("urtrends/providers.db")
                    .display()
            )
        }
    };

    log::info!("Connecting to database: {}", database_url);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    log::info!("Database connected successfully");

    let state = AppState {
        pool,
        summary_cache: Arc::new(std::sync::Mutex::new(None)),
        network_total_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
    };

    log::info!("Starting server on http://0.0.0.0:5001");

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .wrap(middleware::Logger::default())
            .wrap(middleware::DefaultHeaders::new().add(("Cache-Control", "no-cache, no-store, must-revalidate")))
            .service(
                web::scope("")
                    .route("/", web::get().to(dashboard))
                    .service(
                        web::scope("/api")
                            .route("/summary", web::get().to(api_summary))
                            .route("/network_total", web::get().to(api_network_total))
                            .route("/live_total", web::get().to(api_live_total))
                            .route("/regions", web::get().to(api_regions))
                            .route("/at-risk", web::get().to(api_at_risk))
                            .route("/anomalies", web::get().to(api_anomalies))
                            .route("/movers", web::get().to(api_movers))
                            .route("/movers-detailed", web::get().to(api_movers_detailed))
                            .route("/top-countries", web::get().to(api_top_countries))
                            .route("/growth-projection", web::get().to(api_growth_projection))
                            .route("/country-stats/{code}", web::get().to(api_country_stats))
                            .route("/country/{code}", web::get().to(api_country))
                            .route("/churn/{code}", web::get().to(api_churn))
                            .route("/comparison/{code1}/{code2}", web::get().to(api_comparison))
                    )
            )
    })
    .bind("0.0.0.0:5001")?
    .run()
    .await?;

    Ok(())
}

async fn dashboard() -> HttpResponse {
    let html = include_str!("../index.html");
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}
