use std::{collections::HashMap, convert::Infallible, net::SocketAddr};

use http_body_util::Full;
use hyper::{
    body::{Bytes, Incoming},
    header::{HeaderValue, HOST, TRANSFER_ENCODING},
    Request, Response, StatusCode,
};
use tokio::process::Command;

struct Script {
    // Path to the CGI executable
    path: String,

    // URI, empty for "/"
    root: String,

    // Working directory of the CGI executable.
    // If None, base directory of path is used.
    // If path as no base directory, dir is used
    dir: Option<String>,

    // Environment variables
    env: Vec<(String, String)>,

    // Arguments of the CGI executable
    args: Vec<String>,
}

impl Script {
    pub async fn server(
        &self,
        req: Request<Incoming>,
        remote: &SocketAddr,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        let root = if self.root == "" { "/" } else { &self.root };

        if let Some(encoding) = req.headers().get(TRANSFER_ENCODING) {
            if encoding == "chunked" {
                let response = Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from(
                        "Chunked encoding is not supported by CGI.",
                    )))
                    .unwrap();
                return Ok(response);
            }
        }

        let req_path = req.uri().path();
        let path_info = if root != "/" && req_path.starts_with(root) {
            &req_path[root.len()..]
        } else {
            req_path
        };

        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("SERVER_SOFTWARE".to_string(), "cgi-server-rs".to_string());
        env.insert("SERVER_PROTOCOL".to_string(), "HTTP/1.1".to_string());
        env.insert("GATEWAY_INTERFACE".to_string(), "CGI/1.1".to_string());
        if let Some(host) = req.headers().get(HOST) {
            if let Ok(host) = host.to_str() {
                env.insert("HTTP_HOST".to_string(), host.to_string());
                if let Some((hostname, port)) = getHostPort(host) {
                    env.insert("SERVER_NAME".to_string(), hostname.to_string());
                    env.insert("SERVER_PORT".to_string(), port.to_string());
                } else {
                    env.insert("SERVER_NAME".to_string(), host.to_string());
                    env.insert("SERVER_PORT".to_string(), "80".to_string()); // à revoir
                }
            }
        }
        env.insert("REQUEST_METHOD".to_string(), req.method().to_string());
        if let Some(query) = req.uri().query() {
            env.insert("QUERY_STRING".to_string(), query.to_string());
        }
        if let Some(path_and_query) = req.uri().path_and_query() {
            env.insert("REQUEST_URI".to_string(), path_and_query.to_string());
        }
        env.insert("PATH_INFO".to_string(), path_info.to_string());
        env.insert("SCRIPT_NAME".to_string(), root.to_string());
        env.insert("SCRIPT_FILENAME".to_string(), self.path.clone());

        env.insert("REMOTE_ADDR".to_string(), remote.ip().to_string());
        env.insert("REMOTE_HOST".to_string(), remote.ip().to_string());
        env.insert("REMOTE_PORT".to_string(), remote.port().to_string());

        for k in req.headers().keys() {
            let k = k.as_str().to_uppercase();
            let join_str: &str;
            if k == "COOKIE" {
                join_str = ";";
            } else {
                join_str = ",";
            }
            join
        }

        Command::new("echo").arg("coucou").envs(env);

        todo!()
    }
}

fn getHostPort(value: &str) -> Option<(&str, u16)> {
    let split: Vec<&str> = value.split(":").collect();
    if split.len() == 2 {
        Some((split.get(0)?, split.get(1)?.parse().ok()?))
    } else {
        None
    }
}
