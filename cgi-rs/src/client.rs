use std::env;

use hyper::service::Service;
use hyper::Request;
use hyper::Version;

async fn serve<S>(service: S)
where
    S: Service<Request<()>>,
{
    let mut req_builder = Request::builder();

    req_builder = req_builder.method::<&str>(
        &env::var("REQUEST_METHOD").expect("Environment variable REQUEST_METHOD is not defined"),
    );

    let proto =
        env::var("SERVER_PROTOCOL").expect("Environment variable SERVER_PROTOCOL is not defined");

    //req_builder = req_builder.version(proto);

    let req = req_builder.body(()).unwrap();

    service.call(req).await;
}
