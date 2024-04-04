use std::{convert::Infallible, net::SocketAddr};

use beam_lib::AppId;
use clap::Parser;
use reqwest::{Url, header::InvalidHeaderValue};
use tower_http::cors::AllowOrigin;

#[derive(Parser, Clone, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Config {
    /// URL of the Beam Proxy
    #[clap(long, env)]
    pub beam_proxy_url: Url,

    /// Beam AppId of this application
    #[clap(long, env, value_parser = |v: &str| Ok::<_, Infallible>(AppId::new_unchecked(v)))]
    pub beam_app_id: AppId,

    /// Credentials to use on the Beam Proxy
    #[clap(long, env)]
    pub beam_secret: String,

    /// Credentials to use on the Beam Proxy
    #[clap(long, env, value_parser = parse_cors)]
    pub cors_origin: AllowOrigin,

    /// Optional project name used by focus
    #[clap(long, env)]
    pub project: Option<String>,

    /// The socket address this server will bind to
    #[clap(long, env, default_value = "0.0.0.0:8055")]
    pub bind_addr: SocketAddr,

    /// URL to catalogue.json file
    #[clap(long, env)]
    pub catalogue_url: Option<Url>,

    /// URL to prism
    #[clap(long, env, default_value= "http://localhost:8066")]
    pub prism_url: Url
}

fn parse_cors(v: &str) -> Result<AllowOrigin, InvalidHeaderValue> {
    if v == "*" || v.to_lowercase() == "any" {
        Ok(AllowOrigin::any())
    } else {
        v.parse().map(AllowOrigin::exact)
    }
}
