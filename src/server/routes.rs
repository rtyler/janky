/**
 * The routes module contains all the tide routes and the logic to fulfill the responses for each
 * route.
 *
 * Modules are nested for cleaner organization here
 */
use log::*;
use tide::{Body, Request};

use crate::models::Project;
use crate::AppState;

/**
 *  GET /
 */
pub async fn index(req: Request<AppState<'_>>) -> Result<Body, tide::Error> {
    let agents: Vec<String> = req
        .state()
        .agents
        .iter()
        .map(|a| a.render_compact(req.state()))
        .collect();
    let params = json!({
        "page": "home",
        "agents" : agents,
        "config" : req.state().config,
        "projects" : Project::list(&req.state().db).await?,
    });

    debug!("Rendering home page with: {:?}", params);
    let mut body = req.state().render("index", &params).await?;
    body.set_mime("text/html");
    Ok(body)
}

/**
 * GET /project/:name
 */
pub async fn project(req: Request<AppState<'_>>) -> Result<Body, tide::Error> {
    let name: String = req.param("name")?.into();
    let params = json!({
        "name" : name,
    });

    let mut body = req.state().render("project", &params).await?;
    body.set_mime("text/html");
    Ok(body)
}

pub mod api {
    use crate::config::{Scm, Yml};
    use crate::Agent;
    use crate::AppState;
    use log::*;
    use serde::Deserialize;
    use tide::{Request, Response, StatusCode};

    #[derive(Debug, Deserialize)]
    struct RedirectedForm {
        next: Option<String>,
    }

    /**
     *  POST /projects/{name}
     */
    pub async fn execute_project(mut req: Request<AppState<'_>>) -> tide::Result {
        let name: String = req.param("name")?.into();
        let next: RedirectedForm = req.body_form().await?;
        let state = req.state();

        if !state.config.has_project(&name) {
            debug!("Could not find project named: {}", name);
            return Ok(Response::new(StatusCode::NotFound));
        }

        if let Some(project) = state.config.projects.get(&name) {
            match &project.scm {
                Scm::Nonexistent => {
                    info!("Nonexistent SCM, using inline configuration for {}", name);
                    info!("configuration: {:?}", project.inline);
                    if let Some(config) = &project.inline {
                        execute_commands(config, &state.agents).await?;
                        if let Some(red) = &next.next {
                            return Ok(tide::Redirect::new(red).into());
                        }
                    }
                }
                Scm::GitHub {
                    owner,
                    repo,
                    scm_ref,
                } => {
                    let filename = match &project.filename {
                        None => "synchronik.yml".to_string(),
                        Some(filename) => filename.to_string(),
                    };
                    debug!("Fetching the file {} from {}/{}", filename, owner, repo);
                    let res = octocrab::instance()
                        .repos(owner, repo)
                        .raw_file(octocrab::params::repos::Commitish(scm_ref.into()), filename)
                        .await?;
                    let config_file: Yml = serde_yaml::from_str(&res.text().await?)?;
                    debug!("text: {:?}", config_file);
                    execute_commands(&config_file, &state.agents).await?;
                    if let Some(red) = &next.next {
                        return Ok(tide::Redirect::new(red).into());
                    }
                }
            }
            return Ok("{}".into());
        }
        Ok(Response::new(StatusCode::InternalServerError))
    }

    async fn execute_commands(config: &Yml, agents: &Vec<Agent>) -> Result<(), tide::Error> {
        debug!("working {:?}", config);
        for agent in agents {
            debug!("agent: {:?}", agent);
            if agent.can_meet(&config.needs) {
                debug!("agent: {:?} can meet our needs", agent);
                let commands: Vec<synchronik::Command> = config
                    .commands
                    .iter()
                    .map(|c| synchronik::Command::with_script(c))
                    .collect();
                let commands = synchronik::CommandRequest { commands };
                let client = reqwest::Client::new();
                let _res = client
                    .put(
                        agent
                            .url
                            .join("/api/v1/execute")
                            .expect("Failed to join execute URL"),
                    )
                    .json(&commands)
                    .send()
                    .await?;
            }
        }
        Ok(())
    }
}
