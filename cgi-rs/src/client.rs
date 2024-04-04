use std::env;

use bytes::Bytes;
use futures::TryStreamExt;
use http_body_util::BodyStream;
use hyper::body::Body;
use hyper::header;
use hyper::service::Service;
use hyper::Request;
use hyper::Response;
use hyper::Uri;
use hyper::Version;
use tokio::io::stdout;
use tokio::io::AsyncWriteExt;
use tokio::io::BufWriter;

pub async fn run_cgi<S, ResBody>(service: S)
where
    S: Service<Request<()>, Response = Response<ResBody>>,
    ResBody: Body,
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

    let req = req_builder.body(()).unwrap();

    match service.call(req).await {
        Ok(response) => {}
        Err(err) => {}
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

async fn write_response<B: Body<Data = Bytes>>(response: Response<B>) {
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
    .await
    .expect("Cannot write to stdout");
    for (k, v) in response.headers() {
        out.write_all(k.as_str().as_bytes())
            .await
            .expect("Cannot write to stdout");
        out.write_all(": ".as_bytes())
            .await
            .expect("Cannot write to stdout");
        out.write_all(v.as_bytes())
            .await
            .expect("Cannot write to stdout");
        out.write_all("\r\n".as_bytes())
            .await
            .expect("Cannot write to stdout");
    }
    out.write_all("\r\n".as_bytes())
        .await
        .expect("Cannot write to stdout");
    out.flush().await.expect("Cannot flush to stdout");

    let body = BodyStream::new(response.into_body());
    loop {
        //body.try_next()
    }
}
