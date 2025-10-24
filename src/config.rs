use std::{convert::Infallible, net::SocketAddr, path::PathBuf};

use beam_lib::AppId;
use clap::Parser;
use reqwest::{header::InvalidHeaderValue, Url};
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

    /// Where to allow cross-origin resourse sharing from
    #[clap(long, env, value_parser = parse_cors)]
    pub cors_origin: AllowOrigin,

    /// Project name used by Focus
    #[clap(long, env)]
    pub project: String,

    /// Optional transformation format for the results, used by Focus
    #[clap(long, env)]
    pub transform: Option<String>,

    /// The socket address this server will bind to
    #[clap(long, env, default_value = "0.0.0.0:8055")]
    pub bind_addr: SocketAddr,

    /// URL to prism
    #[clap(long, env)]
    pub prism_url: Option<Url>,

    /// Path to a file which will contain the query logs
    #[clap(long, env, value_hint = clap::ValueHint::FilePath)]
    pub log_file: Option<PathBuf>,

    /// Target_application_name
    #[clap(long, env, value_parser, default_value = "focus")]
    pub target_app: String,

    /// Comma separated list of base64 encoded queries
    #[clap(long, env, value_parser, value_delimiter = ',')]
    pub query_filter: Option<Vec<String>>,

    /// Comma separated list of sites to query in case of no sites in the query from Lens
    #[clap(long, env, value_parser, value_delimiter = ',')]
    pub sites: Option<Vec<String>>,

    /// Allowed language set for the lens queries. Defaults to all languages if not set.
    #[clap(long, env, value_parser)]
    pub allowed_lang: Option<String>,
}

fn parse_cors(v: &str) -> Result<AllowOrigin, InvalidHeaderValue> {
    if v == "*" || v.to_lowercase() == "any" {
        Ok(AllowOrigin::any())
    } else {
        v.parse().map(AllowOrigin::exact)
    }
}
