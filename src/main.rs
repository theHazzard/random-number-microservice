extern crate futures;
extern crate hyper;
extern crate rand;
#[macro_use]
extern crate serde_derive;
extern crate queryst;
extern crate serde_json;
#[macro_use]
extern crate failure;

use clap::{
    crate_authors, crate_description, crate_name, crate_version, App, AppSettings, Arg, SubCommand,
};
use dotenv::dotenv;
use failure::Error;
use futures::{future, Future, Stream};
use hyper::service::service_fn;
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use log::{debug, info, trace, warn};
use rand::distributions::{Bernoulli, Normal, Uniform};
use rand::Rng;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs::File;
use std::io::{self, Read};
use std::net::SocketAddr;
use std::ops::Range;

#[derive(Deserialize)]
struct Config {
    address: SocketAddr,
}

#[derive(Serialize)]
struct RngResponse {
    value: f64,
}

#[derive(Deserialize)]
#[serde(tag = "distribution", content = "parameters", rename_all = "lowercase")]
enum RngRequest {
    Uniform {
        #[serde(flatten)]
        range: Range<i32>,
    },
    Normal {
        mean: f64,
        std_dev: f64,
    },
    Bernoulli {
        p: f64,
    },
}

fn serialize(format: &str, resp: &RngResponse) -> Result<Vec<u8>, Error> {
    match format {
        "json" => Ok(serde_json::to_vec(resp)?),
        _ => Err(format_err!("unsupported format {}", format)),
    }
}

fn handle_request(request: RngRequest) -> RngResponse {
    let mut rng = rand::thread_rng();
    let value = {
        match request {
            RngRequest::Uniform { range } => rng.sample(Uniform::from(range)) as f64,
            RngRequest::Normal { mean, std_dev } => rng.sample(Normal::new(mean, std_dev)) as f64,
            RngRequest::Bernoulli { p } => rng.sample(Bernoulli::new(p)) as i8 as f64,
        }
    };
    RngResponse { value }
}

fn microservice_handler(
    req: Request<Body>,
) -> Box<Future<Item = Response<Body>, Error = hyper::Error> + Send> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/random") => {
            let format = {
                let uri = req.uri().query().unwrap_or("");
                let query = queryst::parse(uri).unwrap_or(Value::Null);
                query["format"].as_str().unwrap_or("json").to_string()
            };
            let body = req.into_body().concat2().map(move |chunks| {
                let res = serde_json::from_slice::<RngRequest>(chunks.as_ref())
                    .map(handle_request)
                    .map_err(Error::from)
                    .and_then(move |resp| serialize(&format, &resp));
                match res {
                    Ok(body) => Response::new(body.into()),
                    Err(err) => Response::builder()
                        .status(StatusCode::UNPROCESSABLE_ENTITY)
                        .body(err.to_string().into())
                        .unwrap(),
                }
            });
            Box::new(body)
        }
        _ => {
            let resp = Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body("Not Found".into())
                .unwrap();
            Box::new(future::ok(resp))
        }
    }
}

fn main() {
    let config = File::open("microservice.toml")
        .and_then(|mut file| {
            let mut buffer = String::new();
            file.read_to_string(&mut buffer)?;
            Ok(buffer)
        })
        .and_then(|buffer| {
            toml::from_str::<Config>(&buffer)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))
        })
        .map_err(|err| warn!("Cannot read config file: {}", err))
        .ok();

    let matches = App::new("Server with keys")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("run")
                .about("run the server")
                .arg(
                    Arg::with_name("address")
                        .short("a")
                        .long("address")
                        .takes_value(true)
                        .help("address of the server"),
                )
                .subcommand(
                    SubCommand::with_name("key").about("generates a secret key for cookies"),
                ),
        )
        .get_matches();

    pretty_env_logger::init();
    info!("Rand Microservice - v0.1.0");
    trace!("Starting...");
    let addr = matches
        .value_of("address")
        .map(|s| s.to_owned())
        .or(env::var("ADDRESS").ok())
        .and_then(|addr| addr.parse().ok())
        .or(config.map(|config| config.address))
        .or_else(|| Some(([127, 0, 0, 1], 8080).into()))
        .unwrap();

    debug!("Trying to bind server to address: {}", addr);
    let builder = Server::bind(&addr);
    trace!("Creating service handler...");
    let server = builder.serve(|| service_fn(microservice_handler));
    info!("Used address: {}", server.local_addr());
    let server = server.map_err(drop);
    debug!("Run!");
    hyper::rt::run(server);
}
