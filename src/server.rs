use std::{
    borrow::Cow, collections::HashMap, convert::Infallible, future::ready, net::SocketAddr,
    path::Path, process::Stdio,
};

use hershell::process::{self, ProcStreamExt};
use http_body_util::{combinators::BoxBody, BodyStream, Full, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, TRANSFER_ENCODING},
    Request, Response, StatusCode,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::{ChildStdin, Command},
    task::JoinHandle,
};

use futures::{stream, Stream, StreamExt, TryStreamExt};
use tokio_util::io::{ReaderStream, StreamReader};

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

    // Inherited environment variables
    inherited_env: Vec<String>,
}

impl Script {
    pub async fn server(
        &self,
        req: Request<Incoming>,
        remote: &SocketAddr,
    ) -> Result<Response<BoxBody<Bytes, std::io::Error>>, Infallible> {
        let root = if self.root == "" { "/" } else { &self.root };

        if let Some(encoding) = req.headers().get(TRANSFER_ENCODING) {
            if encoding == "chunked" {
                return Ok(get_error_response_stream(
                    StatusCode::BAD_REQUEST,
                    "Chunked encoding is not supported by CGI.".to_string(),
                ));
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
                if let Some((hostname, port)) = get_host_port(host) {
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
            if k == "PROXY" {
                continue;
            }
            let join_str: &str;
            if k == "COOKIE" {
                join_str = ";";
            } else {
                join_str = ",";
            }
            let mut iter = req.headers().get_all(&k).into_iter();
            if let Some(Ok(first)) = iter.next().map(|e| e.to_str()) {
                let vs = iter.fold(first.to_string(), |s, hv| {
                    if let Ok(h) = hv.to_str() {
                        s + join_str + h
                    } else {
                        s
                    }
                });
                env.insert("HTTP_".to_string() + &k, vs);
            }
        }

        if let Some(cl) = req
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
        {
            env.insert("CONTENT_LENGTH".to_string(), cl.to_string());
        }

        if let Some(ct) = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
        {
            if !ct.is_empty() {
                env.insert("CONTENT_TYPE".to_string(), ct.to_string());
            }
        }

        if let Ok(env_path) = std::env::var("PATH") {
            if !env_path.is_empty() {
                env.insert("PATH".to_string(), env_path);
            } else {
                env.insert(
                    "PATH".to_string(),
                    "/bin:/usr/bin:/usr/ucb:/usr/bsd:/usr/local/bin".to_string(),
                );
            }
        } else {
            env.insert(
                "PATH".to_string(),
                "/bin:/usr/bin:/usr/ucb:/usr/bsd:/usr/local/bin".to_string(),
            );
        }

        for e in &self.inherited_env {
            if let Ok(k) = std::env::var(e) {
                if !k.is_empty() {
                    env.insert(e.clone(), k);
                }
            }
        }

        for e in OS_SPECIFIC_VARS {
            if let Ok(k) = std::env::var(e) {
                if !k.is_empty() {
                    env.insert(e.to_string(), k);
                }
            }
        }

        for (k, v) in &self.env {
            env.insert(k.clone(), v.clone());
        }

        let cwd_cow: Cow<str>;
        let path_cow: Cow<str>;

        if let Some(dir) = &self.dir {
            cwd_cow = Cow::from(dir);
            path_cow = Cow::from(&self.path)
        } else {
            let p = Path::new(&self.path);
            if let Some(parent) = p.parent() {
                let parent_path = parent.to_string_lossy();
                if parent_path.is_empty() {
                    cwd_cow = Cow::from(".")
                } else {
                    cwd_cow = parent_path;
                }
            } else {
                cwd_cow = Cow::from(".");
            }
            if let Some(filename) = p.file_name() {
                path_cow = filename.to_string_lossy();
            } else {
                return Ok(get_error_response_stream(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Cannot use {} as path", &self.path),
                ));
            }
        }

        let cwd: &str = &cwd_cow;
        let path: &str = &path_cow;

        let body = BodyStream::new(req.into_body())
            .try_filter_map(|f| ready(Ok(f.into_data().ok())))
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err));

        let child_opt = Command::new(path)
            .kill_on_drop(true)
            .current_dir(cwd)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env)
            .spawn();

        if let Err(err) = child_opt {
            return Ok(get_error_response_stream(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot run cgi executable with error: {}", err),
            ));
        }

        let child = child_opt.unwrap();

        let process_stream = process::new_process_typed(child, body, 1024).stdout();

        let mut process_reader = StreamReader::new(process_stream);

        let mut line = String::new();

        let response_builder = Response::builder();

        loop {
            match process_reader.read_line(&mut line).await {
                Ok(_) => todo!(),
                Err(_) => todo!(),
            }
        }

        let remaining_stream = ReaderStream::new(process_reader).map_ok(|bytes| Frame::data(bytes));

        Ok(response_builder
            .body(BoxBody::new(StreamBody::new(remaining_stream)))
            .unwrap())
    }
}

fn get_host_port(value: &str) -> Option<(&str, u16)> {
    let split: Vec<&str> = value.split(":").collect();
    if split.len() == 2 {
        Some((split.get(0)?, split.get(1)?.parse().ok()?))
    } else {
        None
    }
}

fn get_error_response(
    code: impl Into<StatusCode>,
    msg: String,
) -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(code)
        .body(BoxBody::new(Full::new(Bytes::from(msg))))
        .unwrap()
}

fn get_error_response_stream(
    code: impl Into<StatusCode>,
    msg: String,
) -> Response<BoxBody<Bytes, std::io::Error>> {
    let content = stream::once(ready(Ok(Frame::data(Bytes::from(msg)))));
    Response::builder()
        .status(code)
        .body(BoxBody::new(StreamBody::new(content)))
        .unwrap()
}

async fn flatten_join_handle<T>(handle: JoinHandle<Result<T, String>>) -> Result<T, String> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(format!("handling failed: {}", err)),
    }
}

async fn feed_stdin(mut stdin: ChildStdin, mut body: BodyStream<Incoming>) -> Result<(), String> {
    while let Some(frame_opt) = body.next().await {
        match frame_opt {
            Ok(frame) => {
                if let Ok(bytes) = frame.into_data() {
                    if let Err(err) = stdin.write_all(&bytes).await {
                        return Err(format!(
                            "Error while writing body to the CGI executable: {}",
                            err
                        ));
                    }
                }
            }
            Err(e) => return Err(format!("Error while reading request body: {}", e)),
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
static OS_SPECIFIC_VARS: &[&str] = &["DYLD_LIBRARY_PATH"];
#[cfg(target_os = "ios")]
static OS_SPECIFIC_VARS: &[&str] = &["DYLD_LIBRARY_PATH"];
#[cfg(target_os = "linux")]
static OS_SPECIFIC_VARS: &[&str] = &["LD_LIBRARY_PATH"];
#[cfg(target_os = "freebsd")]
static OS_SPECIFIC_VARS: &[&str] = &["LD_LIBRARY_PATH"];
#[cfg(target_os = "netbsd")]
static OS_SPECIFIC_VARS: &[&str] = &["LD_LIBRARY_PATH"];
#[cfg(target_os = "openbsd")]
static OS_SPECIFIC_VARS: &[&str] = &["LD_LIBRARY_PATH"];
#[cfg(target_os = "hpux")]
static OS_SPECIFIC_VARS: &[&str] = &["LD_LIBRARY_PATH", "SHLIB_PATH"];
#[cfg(target_os = "irix")]
static OS_SPECIFIC_VARS: &[&str] = &["LD_LIBRARY_PATH", "LD_LIBRARYN32_PATH", "LD_LIBRARY64_PATH"];
#[cfg(target_os = "illumos")]
static OS_SPECIFIC_VARS: &[&str] = &[
    "LD_LIBRARY_PATH",
    "LD_LIBRARY_PATH_32",
    "LD_LIBRARY_PATH_64",
];
#[cfg(target_os = "solaris")]
static OS_SPECIFIC_VARS: &[&str] = &[
    "LD_LIBRARY_PATH",
    "LD_LIBRARY_PATH_32",
    "LD_LIBRARY_PATH_64",
];
#[cfg(target_os = "windows")]
static OS_SPECIFIC_VARS: &[&str] = &["SystemRoot", "COMSPEC", "PATHEXT", "WINDIR"];
