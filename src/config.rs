use std::net::SocketAddr;

use clap::Parser;
use reqwest::{Url, header::InvalidHeaderValue};
use tower_http::cors::AllowOrigin;

#[derive(Parser, Clone, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Config {
    /// Url of the local blaze server including /fhir/
    #[clap(long, env)]
    pub blaze_url: Url,

    /// Where to allow cross-origin resourse sharing from
    #[clap(long, env, value_parser = parse_cors)]
    pub cors_origin: AllowOrigin,

    /// The socket address this server will bind to
    #[clap(long, env, default_value = "0.0.0.0:8055")]
    pub bind_addr: SocketAddr,
}

fn parse_cors(v: &str) -> Result<AllowOrigin, InvalidHeaderValue> {
    if v == "*" || v.to_lowercase() == "any" {
        Ok(AllowOrigin::any())
    } else {
        v.parse().map(AllowOrigin::exact)
    }
}
