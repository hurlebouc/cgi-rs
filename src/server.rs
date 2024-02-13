use std::{collections::HashMap, convert::Infallible};

use http_body_util::Full;
use hyper::{
    body::{Bytes, Incoming},
    header::{HOST, TRANSFER_ENCODING},
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
    async fn server(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
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

        let port = req.uri().port_u16().unwrap_or(80);

        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("SERVER_SOFTWARE".to_string(), "cgi-server-rs".to_string());
        env.insert("SERVER_PROTOCOL".to_string(), "HTTP/1.1".to_string());
        env.insert("GATEWAY_INTERFACE".to_string(), "CGI/1.1".to_string());
        if let Some(host) = req.uri().host() {
            env.insert("HTTP_HOST".to_string(), host.to_string());
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
        env.insert("SERVER_PORT".to_string(), port.to_string());

        Command::new("echo").arg("coucou").envs(env);

        todo!()
    }
}
