use std::env;
use std::io;
use std::pin::pin;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use bytes::Bytes;
use futures::stream;
use futures::StreamExt;
use futures::TryStreamExt;
use http_body_util::BodyStream;
use http_body_util::StreamBody;
use hyper::body::Body;
use hyper::body::Frame;
use hyper::header;
use hyper::service::Service;
use hyper::Request;
use hyper::Response;
use hyper::Uri;
use hyper::Version;
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
    let mut req_builder = Request::builder();

    req_builder = req_builder.method::<&str>(
        &env::var("REQUEST_METHOD").expect("Environment variable REQUEST_METHOD is not defined"),
    );

    // Cannot create version from string
    let _proto =
        env::var("SERVER_PROTOCOL").expect("Environment variable SERVER_PROTOCOL is not defined");
    req_builder = req_builder.version(Version::default());

    match env::var("HTTP_HOST") {
        Ok(host) => {
            req_builder = req_builder.header(header::HOST, host);
        }
        Err(env::VarError::NotUnicode(os_string)) => {
            panic!("Cannot read {} as host value", os_string.to_string_lossy())
        }

        Err(env::VarError::NotPresent) => {}
    }

    match env::var("CONTENT_LENGTH") {
        Ok(length) => match length.parse::<u32>() {
            Ok(_) => {
                req_builder = req_builder.header(header::CONTENT_LENGTH, length);
            }
            Err(_) => panic!("Cannot read {} as content-length integer value", length),
        },
        Err(env::VarError::NotPresent) => {}
        Err(env::VarError::NotUnicode(os_string)) => panic!(
            "Cannot read {} as content-length value",
            os_string.to_string_lossy()
        ),
    }

    match env::var("CONTENT_TYPE") {
        Ok(ct) => {
            req_builder = req_builder.header(header::CONTENT_TYPE, ct);
        }
        Err(env::VarError::NotPresent) => {}
        Err(env::VarError::NotUnicode(os_string)) => panic!(
            "Cannot read {} as content-type value",
            os_string.to_string_lossy()
        ),
    }

    for (k, v) in env::vars() {
        if k.starts_with("HTTP_") {
            req_builder = req_builder.header(&k[5..].replace("_", "-"), v);
        }
    }

    match env::var("REQUEST_URI") {
        Ok(request_uri) => match Uri::try_from(&request_uri) {
            Ok(uri) => req_builder = req_builder.uri(uri),
            Err(_) => panic!("Cannot read REQUEST_URI ({}) as valid URI", &request_uri),
        },
        Err(env::VarError::NotPresent) => match Uri::try_from(get_req_uri()) {
            Ok(uri) => req_builder = req_builder.uri(uri),
            Err(_) => panic!(
                "Cannot read SCRIPT_NAME + PATH_INFO + ? + QUERY_STRING ({}) as valid URI",
                get_req_uri()
            ),
        },
        Err(env::VarError::NotUnicode(os_string)) => {
            panic!("Cannot read {} as URI value", os_string.to_string_lossy())
        }
    }

    let req = req_builder.body(StdinBody::new()).unwrap();

    let conn_info = ConnInfo {
        local_addr: match env::var("SERVER_NAME") {
            Ok(name) => name,
            Err(env::VarError::NotPresent) => panic!("Missing variable SERVER_NAME"),
            Err(env::VarError::NotUnicode(os_string)) => {
                panic!("Cannot read {} as server name", os_string.to_string_lossy())
            }
        },
        remote_addr: match env::var("REMOTE_ADDR") {
            Ok(addr) => match addr.parse() {
                Ok(addr) => addr,
                Err(_) => panic!("Cannot parse {} as IP address", addr),
            },
            Err(env::VarError::NotPresent) => panic!("Missing variable REMOTE_ADDR"),
            Err(env::VarError::NotUnicode(os_string)) => panic!(
                "Cannot read {} as remote IP address",
                os_string.to_string_lossy()
            ),
        },
        remote_port: match env::var("REMOTE_PORT") {
            Ok(port) => match port.parse() {
                Ok(port) => port,
                Err(_) => panic!("Cannot parse {} as remote port", port),
            },
            Err(env::VarError::NotPresent) => panic!("Missing variable REMOTE_PORT"),
            Err(env::VarError::NotUnicode(os_string)) => {
                panic!("Cannot read {} as remote port", os_string.to_string_lossy())
            }
        },
        local_port: match env::var("SERVER_PORT") {
            Ok(port) => match port.parse() {
                Ok(port) => port,
                Err(_) => panic!("Cannot parse {} as local port", port),
            },
            Err(env::VarError::NotPresent) => panic!("Missing variable SERVER_PORT"),
            Err(env::VarError::NotUnicode(os_string)) => {
                panic!("Cannot read {} as local port", os_string.to_string_lossy())
            }
        },
    };

    let service = service_builder(conn_info);

    match service.call(req).await {
        Ok(response) => write_response(response)
            .await
            .expect("Cannot write to stdout"),
        Err(err) => {
            panic!("cannot call service")
        }
    }
}

fn get_req_uri() -> String {
    env::var("SCRIPT_NAME").unwrap_or_default()
        + &env::var("PATH_INFO").unwrap_or_default()
        + &match env::var("QUERY_STRING") {
            Ok(query) => "?".to_string() + &query,
            Err(env::VarError::NotPresent) => "".to_string(),
            Err(env::VarError::NotUnicode(os_string)) => {
                panic!("Cannot read {} as query value", os_string.to_string_lossy())
            }
        }
}

async fn write_response<Data: AsRef<[u8]>, B: Body<Data = Data>>(
    response: Response<B>,
) -> io::Result<()> {
    let mut out = BufWriter::new(stdout());
    let code = response.status().as_u16();
    let reason = response.status().canonical_reason();
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
