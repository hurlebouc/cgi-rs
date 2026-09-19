use std::env;
use std::io;
use std::pin::pin;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use http_body_util::BodyStream;
use hyper::body::Body;
use hyper::body::Frame;
use hyper::header;
use hyper::service::Service;
use hyper::Request;
use hyper::Response;
use hyper::Uri;
use hyper::Version;
use log::debug;
use tokio::io::stdin;
use tokio::io::stdout;
use tokio::io::AsyncWriteExt;
use tokio::io::BufWriter;
use tokio::io::Stdin;
use tokio_util::io::ReaderStream;

use crate::common::ConnInfo;

pub struct StdinBody {
    body: ReaderStream<Stdin>,
}

impl StdinBody {
    fn new() -> StdinBody {
        StdinBody {
            body: ReaderStream::new(stdin()),
        }
    }
}

impl Body for StdinBody {
    type Data = Bytes;

    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.body.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(Frame::data(bytes)))),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub async fn run_cgi<S, F, ResBody>(service_builder: F)
where
    S: Service<Request<StdinBody>, Response = Response<ResBody>>,
    F: FnOnce(ConnInfo) -> S,
    ResBody: Body,
    ResBody::Data: AsRef<[u8]>,
{
    if let Err(err) = run_cgi_inner(service_builder).await {
        write_error_response(&err).await;
    }
}

async fn run_cgi_inner<S, F, ResBody>(service_builder: F) -> Result<(), CgiClientError>
where
    S: Service<Request<StdinBody>, Response = Response<ResBody>>,
    F: FnOnce(ConnInfo) -> S,
    ResBody: Body,
    ResBody::Data: AsRef<[u8]>,
{
    debug!("Process new CGI request");
    let mut req_builder = Request::builder();

    let method = read_var("REQUEST_METHOD")?;
    req_builder = req_builder.method::<&str>(&method);

    let _proto = read_var("SERVER_PROTOCOL")?;
    req_builder = req_builder.version(Version::default());

    match env::var("HTTP_HOST") {
        Ok(host) => {
            debug!("HTTP_HOST: {}", &host);
            req_builder = req_builder.header(header::HOST, host);
        }
        Err(env::VarError::NotUnicode(os_string)) => {
            return Err(CgiClientError::new(format!(
                "Cannot read {} as host value",
                os_string.to_string_lossy()
            )));
        }
        Err(env::VarError::NotPresent) => {}
    }

    match env::var("CONTENT_LENGTH") {
        Ok(length) => {
            debug!("CONTENT_LENGTH: {}", &length);
            if !length.trim().is_empty() {
                match length.parse::<u64>() {
                    Ok(_) => {
                        req_builder = req_builder.header(header::CONTENT_LENGTH, length);
                    }
                    Err(_) => {
                        return Err(CgiClientError::new(format!(
                            "Cannot read {} as content-length integer value",
                            length
                        )));
                    }
                }
            }
        }
        Err(env::VarError::NotPresent) => {}
        Err(env::VarError::NotUnicode(os_string)) => {
            return Err(CgiClientError::new(format!(
                "Cannot read {} as content-length value",
                os_string.to_string_lossy()
            )));
        }
    }

    match env::var("CONTENT_TYPE") {
        Ok(ct) => {
            debug!("CONTENT_TYPE: {}", &ct);
            req_builder = req_builder.header(header::CONTENT_TYPE, ct);
        }
        Err(env::VarError::NotPresent) => {}
        Err(env::VarError::NotUnicode(os_string)) => {
            return Err(CgiClientError::new(format!(
                "Cannot read {} as content-type value",
                os_string.to_string_lossy()
            )));
        }
    }

    for (k, v) in env::vars() {
        debug!("ENV => {}: {}", &k, &v);
        if k.starts_with("HTTP_") {
            req_builder = req_builder.header(&k[5..].replace("_", "-"), v);
        }
    }

    match env::var("REQUEST_URI") {
        Ok(request_uri) => {
            debug!("REQUEST_URI: {}", &request_uri);
            match Uri::try_from(&request_uri) {
                Ok(uri) => req_builder = req_builder.uri(uri),
                Err(_) => {
                    return Err(CgiClientError::new(format!(
                        "Cannot read REQUEST_URI ({}) as valid URI",
                        &request_uri
                    )));
                }
            }
        }
        Err(env::VarError::NotPresent) => match Uri::try_from(get_req_uri()?) {
            Ok(uri) => req_builder = req_builder.uri(uri),
            Err(_) => {
                return Err(CgiClientError::new(format!(
                    "Cannot read SCRIPT_NAME + PATH_INFO + ? + QUERY_STRING ({}) as valid URI",
                    get_req_uri().unwrap_or_default()
                )));
            }
        },
        Err(env::VarError::NotUnicode(os_string)) => {
            return Err(CgiClientError::new(format!(
                "Cannot read {} as URI value",
                os_string.to_string_lossy()
            )));
        }
    }

    let req = req_builder
        .body(StdinBody::new())
        .map_err(|err| CgiClientError::new(format!("Cannot build request: {}", err)))?;

    let conn_info = ConnInfo {
        local_addr: read_var("SERVER_NAME")?,
        remote_addr: match read_var("REMOTE_ADDR")?.parse() {
            Ok(addr) => addr,
            Err(_) => {
                return Err(CgiClientError::new(format!(
                    "Cannot parse {} as IP address",
                    env::var("REMOTE_ADDR").unwrap_or_default()
                )));
            }
        },
        remote_port: read_port("REMOTE_PORT")?,
        local_port: read_port("SERVER_PORT")?,
    };

    let service = service_builder(conn_info);

    let response = service
        .call(req)
        .await
        .map_err(|_| CgiClientError::new("cannot call service".to_string()))?;

    write_response(response).await?;
    Ok(())
}

fn read_var(name: &str) -> Result<String, CgiClientError> {
    match env::var(name) {
        Ok(v) => Ok(v),
        Err(env::VarError::NotPresent) => {
            Err(CgiClientError::new(format!("Missing variable {}", name)))
        }
        Err(env::VarError::NotUnicode(os_string)) => Err(CgiClientError::new(format!(
            "Cannot read {} as {} value",
            os_string.to_string_lossy(),
            name
        ))),
    }
}

fn read_port(name: &str) -> Result<u16, CgiClientError> {
    let port = read_var(name)?;
    match port.parse::<u16>() {
        Ok(p) => Ok(p),
        Err(_) => Err(CgiClientError::new(format!(
            "Cannot parse {} as {} integer value",
            port, name
        ))),
    }
}

#[derive(Debug)]
pub struct CgiClientError {
    message: String,
}

impl CgiClientError {
    fn new(message: String) -> CgiClientError {
        CgiClientError { message }
    }
}

impl std::fmt::Display for CgiClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CgiClientError {}

impl From<std::io::Error> for CgiClientError {
    fn from(value: std::io::Error) -> Self {
        CgiClientError::new(format!("I/O error: {}", value))
    }
}

fn get_req_uri() -> Result<String, CgiClientError> {
    let query = match env::var("QUERY_STRING") {
        Ok(query) => "?".to_string() + &query,
        Err(env::VarError::NotPresent) => "".to_string(),
        Err(env::VarError::NotUnicode(os_string)) => {
            return Err(CgiClientError::new(format!(
                "Cannot read {} as query value",
                os_string.to_string_lossy()
            )));
        }
    };
    let res = env::var("SCRIPT_NAME").unwrap_or_default()
        + &env::var("PATH_INFO").unwrap_or_default()
        + &query;
    debug!("get_req_uri(): {}", &res);
    Ok(res)
}

async fn write_error_response(err: &CgiClientError) {
    eprintln!("CGI client error: {}", err);
    let mut out = BufWriter::new(stdout());
    let _ = out
        .write_all(b"Status: 500 Internal Server Error\r\n")
        .await;
    let _ = out.write_all(b"Content-Type: text/plain\r\n").await;
    let _ = out.write_all(b"\r\n").await;
    let _ = out.write_all(b"Internal server error").await;
    let _ = out.flush().await;
}

async fn write_response<Data: AsRef<[u8]>, B: Body<Data = Data>>(
    response: Response<B>,
) -> io::Result<()> {
    let mut out = BufWriter::new(stdout());
    let code = response.status().as_u16();
    debug!("STATUS: {}", code);
    let reason = response.status().canonical_reason();
    debug!("REASON: {:?}", &reason);
    out.write_all(
        format!(
            "Status: {} {}\r\n",
            code,
            reason.unwrap_or("unknown reason")
        )
        .as_bytes(),
    )
    .await?;
    for (k, v) in response.headers() {
        debug!("RESPONSE => {}: {:?}", &k, &v);
        out.write_all(k.as_str().as_bytes()).await?;
        out.write_all(": ".as_bytes()).await?;
        out.write_all(v.as_bytes()).await?;
        out.write_all("\r\n".as_bytes()).await?;
    }
    out.write_all("\r\n".as_bytes()).await?;
    out.flush().await?;

    let body = pin!(response.into_body());
    let mut stream_body = BodyStream::new(body);
    while let Ok(Some(frame)) = stream_body.try_next().await {
        match frame.into_data() {
            Ok(data) => {
                out.write_all(data.as_ref()).await?;
                out.flush().await?;
            }
            Err(_) => {}
        }
    }
    Ok(())
}
