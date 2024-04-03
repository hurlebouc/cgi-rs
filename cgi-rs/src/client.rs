use std::env;

use hyper::body::Body;
use hyper::service::Service;
use hyper::Request;
use hyper::Response;
use hyper::Version;

async fn runCGI<S, ResBody>(service: S)
where
    S: Service<Request<()>, Response = Response<ResBody>>,
    ResBody: Body,
{
    let mut req_builder = Request::builder();

    req_builder = req_builder.method::<&str>(
        &env::var("REQUEST_METHOD").expect("Environment variable REQUEST_METHOD is not defined"),
    );

    let proto =
        env::var("SERVER_PROTOCOL").expect("Environment variable SERVER_PROTOCOL is not defined");

    //req_builder = req_builder.version(proto);

    let req = req_builder.body(()).unwrap();

    match service.call(req).await {
        Ok(response) => {}
        Err(err) => {}
    }
}
