/*
 * This is the main Janky entrypoint for the server"
 */

#[macro_use]
extern crate serde_json;

use async_std::sync::{Arc, RwLock};
use dotenv::dotenv;
use handlebars::Handlebars;
use log::*;
use sqlx::SqlitePool;

#[derive(Clone, Debug)]
pub struct AppState<'a> {
    pub db: SqlitePool,
    hb: Arc<RwLock<Handlebars<'a>>>,
}

impl AppState<'_> {
    fn new(db: SqlitePool) -> Self {
        Self {
            db: db,
            hb: Arc::new(RwLock::new(Handlebars::new())),
        }
    }

    pub async fn register_templates(&self) -> Result<(), handlebars::TemplateFileError> {
        let mut hb = self.hb.write().await;
        hb.clear_templates();
        hb.register_templates_directory(".hbs", "views")
    }

    pub async fn render(
        &self,
        name: &str,
        data: &serde_json::Value,
    ) -> Result<tide::Body, tide::Error> {
        /*
         * In debug mode, reload the templates on ever render to avoid
         * needing a restart
         */
        #[cfg(debug_assertions)]
        {
            self.register_templates().await;
        }
        let hb = self.hb.read().await;
        let view = hb.render(name, data)?;
        Ok(tide::Body::from_string(view))
    }
}

/**
 * The routes module contains all the tide routes and the logic to fulfill the responses for each
 * route.
 *
 * Modules are nested for cleaner organization here
 */
mod routes {
    use crate::AppState;
    use log::*;
    use std::collections::HashMap;
    use tide::{Body, Request, StatusCode};
    use uuid::Uuid;

    /**
     *  GET /
     */
    pub async fn index(req: Request<AppState<'_>>) -> Result<Body, tide::Error> {
        let params = json!({
            "page": "home"
        });
        let mut body = req.state().render("index", &params).await?;
        body.set_mime("text/html");
        Ok(body)
    }

    pub mod api {
        use log::*;
        use tide::{Body, Request, Response, StatusCode};

        use crate::AppState;
    }
}

#[async_std::main]
async fn main() -> Result<(), tide::Error> {
    pretty_env_logger::init();
    dotenv().ok();

    let database_url = std::env::var("DATABASE_URL").unwrap_or(":memory:".to_string());
    let state = AppState::new(
        SqlitePool::connect(&database_url)
            .await
            .expect("Failed to open SqlitePool"),
    );
    state.register_templates().await;
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
    app.at("/static").serve_dir("static/")?;
    debug!("Configuring routes");
    app.at("/").get(routes::index);
    app.listen("0.0.0.0:8000").await?;
    Ok(())
}