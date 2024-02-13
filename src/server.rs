use std::{
    convert::Infallible,
    path::{Path, PathBuf},
};

use http_body_util::Full;
use hyper::{
    body::{Bytes, Incoming},
    header::TRANSFER_ENCODING,
    Request, Response, StatusCode,
};
use regex::Regex;

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

        let re = Regex::new(r"Hello (?<name>\w+)!").unwrap();

        todo!()
    }
}
