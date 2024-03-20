use std::net::SocketAddr;

use std::path::PathBuf;
use std::time::Duration;

use cgi_rs::limit::GlobalHttpConcurrencyLimitLayer;
use cgi_rs::server::Script;
use cgi_rs::timeout::RequestBodyTimeoutLayer;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::timeout::ResponseBodyTimeoutLayer;

use clap::Parser;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Listening port (default 8080)
    #[arg(short, long)]
    port: Option<u16>,

    /// Path of cgi script
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port.unwrap_or(8080)));
    let script = Script {
        path: args.path.to_string_lossy().to_string(),
        root: "".to_string(),
        dir: None,
        env: Vec::new(),
        args: Vec::new(),
        inherited_env: Vec::new(),
    };
    //let semaphore = Arc::new(Semaphore::new(1));
    // let concurrence_layer = GlobalConcurrencyLimitLayer::new(1);
    let concurrence_layer = GlobalHttpConcurrencyLimitLayer::new(2);

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;

    // We start a loop to continuously accept incoming connections
    loop {
        let script = script.clone();
        //let semaphore = semaphore.clone();
        let concurrence_layer = concurrence_layer.clone();
        let (stream, remote) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);
        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            let service = ServiceBuilder::new()
                .layer(concurrence_layer)
                .layer(RequestBodyTimeoutLayer::new(Duration::from_secs(30)))
                .layer(ResponseBodyTimeoutLayer::new(Duration::from_secs(30)))
                //.service_fn(handle);
                .service_fn(|req| script.server(req, remote));
            //.service(script.service(remote));
            // Finally, we bind the incoming connection to our `hello` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                //.serve_connection(io, script.service_hyper(remote))
                // .serve_connection(
                //     io,
                //     service_fn(|req| async {
                //         let permit = semaphore.clone().acquire_owned().await.unwrap();
                //         script
                //             .server(req, remote)
                //             .await
                //             .map(|resp| resp.map(|body| PermittedBody::new(permit, body)))
                //     }),
                // )
                .serve_connection(io, hyper_util::service::TowerToHyperService::new(service))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}
