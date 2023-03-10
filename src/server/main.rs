/*
 * This is the main Synchronik entrypoint for the server
 */

#[macro_use]
extern crate serde_json;

use std::path::PathBuf;

use async_std::sync::{Arc, RwLock};
use dotenv::dotenv;
use gumdrop::Options;
use handlebars::Handlebars;
use log::*;
use sqlx::SqlitePool;
use url::Url;

mod config;
mod models;
mod routes;

use crate::config::*;
use crate::models::Project;

#[derive(Clone, Debug)]
pub struct AppState<'a> {
    pub db: SqlitePool,
    pub config: ServerConfig,
    pub agents: Vec<Agent>,
    hb: Arc<RwLock<Handlebars<'a>>>,
}

impl AppState<'_> {
    fn new(db: SqlitePool, config: ServerConfig) -> Self {
        let mut hb = Handlebars::new();

        #[cfg(debug_assertions)]
        hb.set_dev_mode(true);

        Self {
            db,
            config,
            agents: vec![],
            hb: Arc::new(RwLock::new(hb)),
        }
    }

    pub async fn register_templates(&self) -> Result<(), handlebars::TemplateError> {
        let mut hb = self.hb.write().await;
        hb.clear_templates();
        hb.register_templates_directory(".hbs", "views")
    }

    pub async fn render(
        &self,
        name: &str,
        data: &serde_json::Value,
    ) -> Result<tide::Body, tide::Error> {
        let hb = self.hb.read().await;
        let view = hb.render(name, data)?;
        Ok(tide::Body::from_string(view))
    }
}

#[derive(Debug, Options)]
struct ServerOptions {
    #[options(help = "print help message")]
    help: bool,
    #[options(help = "host:port to bind the server to", default = "0.0.0.0:8000")]
    listen: String,
    #[options(help = "Path to the configuration file")]
    config: Option<PathBuf>,
    #[options(help = "Comma separated list of URLs for agents")]
    agents: Vec<Url>,
}

#[async_std::main]
async fn main() -> Result<(), tide::Error> {
    pretty_env_logger::init();
    dotenv().ok();
    let opts = ServerOptions::parse_args_default_or_exit();
    debug!("Starting with options: {:?}", opts);

    let config = match opts.config {
        Some(path) => ServerConfig::from_path(&path)?,
        None => ServerConfig::default(),
    };
    debug!("Starting with config: {:?}", config);

    let database_url = std::env::var("DATABASE_URL").unwrap_or(":memory:".to_string());
    let pool = SqlitePool::connect(&database_url).await?;

    /* If synchronik-server is running in memory, make sure the database is set up properly */
    if database_url == ":memory:" {
        sqlx::migrate!().run(&pool).await?;
    }
    let mut state = AppState::new(pool.clone(), config.clone());

    /*
     * Make sure the database has all the projects configured
     */

    for name in config.projects.keys() {
        match Project::by_name(name, &pool).await {
            Ok(_) => {}
            Err(sqlx::Error::RowNotFound) => {
                debug!("Project not found in database, creating: {}", name);
                Project::create(&Project::new(name), &pool).await?;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    for (name, agent) in config.agents.iter() {
        debug!("Requesting capabilities from agent: {} {:?}", name, agent);
        let response: synchronik::CapsResponse =
            reqwest::get(agent.url.join("/api/v1/capabilities")?)
                .await?
                .json()
                .await?;
        state.agents.push(Agent::new(
            name.to_string(),
            agent.url.clone(),
            response.caps,
        ));
    }

    state
        .register_templates()
        .await
        .expect("Failed to register handlebars templates");
    let mut app = tide::with_state(state);

    #[cfg(not(debug_assertions))]
    {
        info!("Activating RELEASE mode configuration");
        app.with(driftwood::ApacheCombinedLogger);
    }

    #[cfg(debug_assertions)]
    {
        info!("Activating DEBUG mode configuration");
        info!("Enabling a very liberal CORS policy for debug purposes");
        use tide::security::{CorsMiddleware, Origin};
        let cors = CorsMiddleware::new()
            .allow_methods(
                "GET, POST, PUT, OPTIONS"
                    .parse::<tide::http::headers::HeaderValue>()
                    .unwrap(),
            )
            .allow_origin(Origin::from("*"))
            .allow_credentials(false);

        app.with(cors);
    }
    /*
     * All builds will have apidocs, since they're handy
     */
    app.at("/apidocs").serve_dir("apidocs/")?;
    app.at("/static").serve_dir("static/")?;

    debug!("Configuring routes");
    app.at("/").get(routes::index);
    app.at("/project/:name").get(routes::project);

    debug!("Configuring API routes");
    app.at("/api/v1/projects/:name")
        .post(routes::api::execute_project);
    app.listen(opts.listen).await?;
    Ok(())
}
